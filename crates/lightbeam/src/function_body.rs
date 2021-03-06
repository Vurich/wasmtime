#[cfg(debug_assertions)]
use crate::backend::Registers;
use crate::{
    backend::{
        ret_locs, BrAction, CCLoc, CallingConvention, CodeGenSession, Label, MaybeCCLoc, Target,
        ValueLocation,
    },
    error::{error, error_nopanic, Error},
    microwasm::*,
    module::{ModuleContext, SigType, Signature},
};
use cranelift_codegen::{binemit, ir};
use dynasmrt::DynasmApi;
#[cfg(debug_assertions)]
use more_asserts::{assert_ge, debug_assert_ge};
use more_asserts::{assert_le, debug_assert_le};
use std::{
    collections::HashMap, convert::TryFrom, fmt, hash::Hash, iter, mem, ops::RangeInclusive,
};
use wasmparser::FunctionBody;

#[derive(Debug)]
struct Block {
    label: BrTarget<Label>,
    calling_convention: Option<CallingConvention>,
    params: u32,
    // TODO: Is there a cleaner way to do this? `has_backwards_callers` should always be set if `is_next`
    //       is false, so we should probably use an `enum` here.
    is_next: bool,
    already_emitted: bool,
    num_callers: NumCallers,
    actual_num_callers: NumCallers,
    has_backwards_callers: bool,
}

pub trait OffsetSink {
    fn offset(
        &mut self,
        offset_in_wasm_function: ir::SourceLoc,
        offset_in_compiled_function: usize,
    );
}

pub struct NullOffsetSink;

impl OffsetSink for NullOffsetSink {
    fn offset(&mut self, _: ir::SourceLoc, _: usize) {}
}

pub struct Sinks<'a> {
    pub relocs: &'a mut dyn binemit::RelocSink,
    pub traps: &'a mut dyn binemit::TrapSink,
    pub offsets: &'a mut dyn OffsetSink,
}

impl Sinks<'_> {
    pub fn reborrow(&mut self) -> Sinks<'_> {
        Sinks {
            relocs: &mut *self.relocs,
            traps: &mut *self.traps,
            offsets: &mut *self.offsets,
        }
    }
}

pub fn translate_wasm<M>(
    session: &mut CodeGenSession<M>,
    sinks: Sinks<'_>,
    func_idx: u32,
    body: FunctionBody<'_>,
) -> Result<(), Error>
where
    M: ModuleContext,
    for<'any> &'any M::Signature: Into<OpSig>,
{
    use itertools::Either;

    let ty = session.module_context.defined_func_type(func_idx);

    let microwasm_conv = MicrowasmConv::new(
        session.module_context,
        ty.params().iter().map(SigType::to_microwasm_type),
        ty.returns().iter().map(SigType::to_microwasm_type),
        body,
        session.pointer_type(),
    )?
    .flat_map(|ops| match ops {
        Ok(ops) => Either::Left(ops.into_iter().map(Ok)),
        Err(e) => Either::Right(iter::once(Err(e))),
    });

    translate(session, sinks, func_idx, microwasm_conv)?;
    Ok(())
}

pub trait MakeInternalLabel: Sized {
    type Id;

    const FIRST_ID: Self::Id;

    fn new_internal(id: Self::Id) -> (Self, Self::Id);
}

pub fn translate<M, I, L: Send + Sync + 'static>(
    session: &mut CodeGenSession<M>,
    mut sinks: Sinks<'_>,
    func_idx: u32,
    body: I,
) -> Result<(), Error>
where
    M: ModuleContext,
    I: IntoIterator<Item = Result<WithLoc<Operator<L>>, Error>>,
    L: Hash + Clone + Eq + MakeInternalLabel + fmt::Debug,
    BrTarget<L>: std::fmt::Display,
{
    fn drop_elements<T>(stack: &mut Vec<T>, depths: std::ops::RangeInclusive<u32>) {
        let _ = (|| {
            let start = stack
                .len()
                .checked_sub(1)?
                .checked_sub(*depths.end() as usize)?;
            let end = stack
                .len()
                .checked_sub(1)?
                .checked_sub(*depths.start() as usize)?;
            let real_range = start..=end;

            stack.drain(real_range);

            Some(())
        })();
    }

    let func_type = session.module_context.defined_func_type(func_idx);
    let mut body = body.into_iter().peekable();

    let mut additional_instructions_offset = Default::default();
    let mut additional_instructions = Vec::<Operator<L>>::new().into_iter();
    let mut internal_block_id = L::FIRST_ID;

    let module_context = &*session.module_context;
    let mut op_offset_map = mem::replace(&mut session.op_offset_map, vec![]);
    {
        let ctx = &mut session.new_context(func_idx, sinks.reborrow());
        op_offset_map.push((
            ctx.asm.offset(),
            Box::new(format!("start .Fn{}:", func_idx)),
        ));

        let params = func_type.params().iter().map(|t| t.to_microwasm_type());
        let returns = func_type.returns().iter().map(|t| t.to_microwasm_type());

        if returns.len() > 1 {
            return Err(error_nopanic("invalid result arity"));
        }

        const DISASSEMBLE: bool = false;

        if DISASSEMBLE {
            print!("     \tdef .Fn{} :: [", func_idx);

            let mut params_iter = params.clone();

            if let Some(arg) = params_iter.next() {
                print!("{}", arg);
                for arg in params_iter {
                    print!(", {}", arg);
                }
            }
            println!("]");

            print!("     \tdef .return :: [");
            let mut returns_iter = returns.clone();

            if let Some(arg) = returns_iter.next() {
                print!("{}", arg);
                for arg in returns_iter {
                    print!(", {}", arg);
                }
            }
            println!("]");

            println!("start .Fn{}:", func_idx);
        }

        ctx.start_function(params, returns.clone())?;

        let mut blocks = HashMap::<BrTarget<L>, Block>::new();

        let ret_block_params = returns.len() as u32;
        let locs = ret_locs(returns).locs;

        blocks.insert(
            BrTarget::Return,
            Block {
                label: BrTarget::Return,
                params: ret_block_params,
                calling_convention: Some(CallingConvention::function_start(locs)),
                is_next: false,
                already_emitted: true,
                has_backwards_callers: false,
                actual_num_callers: Default::default(),
                num_callers: NumCallers::Many,
            },
        );

        #[cfg_attr(not(debug_assertions), allow(unused_variables))]
        let mut in_block = true;

        while let Some(op_offset) = additional_instructions
            .next()
            .map(|op| {
                Ok(WithLoc {
                    op,
                    offset: additional_instructions_offset,
                })
            })
            .or_else(|| body.next())
        {
            let op_offset = op_offset?;

            if DISASSEMBLE {
                println!("{}", DisassemblyOpFormatter(op_offset.clone()));
            }

            let WithLoc { op, offset } = op_offset;

            ctx.set_source_loc(offset);
            ctx.sinks.offsets.offset(offset, ctx.asm.offset().0);

            if let Some(Ok(WithLoc {
                op: Operator::Start(label),
                ..
            })) = body.peek()
            {
                let block = match blocks.get_mut(&BrTarget::Label(label.clone())) {
                    None => return Err(error("Label defined before being declared")),
                    Some(o) => o,
                };
                block.is_next = true;
            }

            // `cfg` on blocks doesn't work in the compiler right now, so we have to write a dummy macro
            #[cfg(debug_assertions)]
            macro_rules! assertions {
                () => {
                    if let Operator::Start(label) = &op {
                        debug_assert!(!in_block);

                        let block = &blocks[&BrTarget::Label(label.clone())];
                        let num_cc_params = block
                            .calling_convention
                            .as_ref()
                            .map(|cc| cc.arguments.locs.len());
                        if let Some(num_cc_params) = num_cc_params {
                            debug_assert_ge!(num_cc_params, block.params as usize);
                        }
                    } else {
                        debug_assert!(in_block);

                        let mut actual_regs = Registers::new();
                        let mut actual_stack = ctx.allocated_stack.clone();
                        actual_stack.clear();
                        actual_stack.set_depth(crate::backend::FUNCTION_START_DEPTH)?;
                        actual_stack.set_depth_and_free(ctx.allocated_stack.stack_depth())?;

                        actual_regs.release_scratch_register()?;
                        for val in &ctx.stack {
                            match *val {
                                ValueLocation::Reg(gpr) => {
                                    actual_regs.mark_used(gpr);
                                }
                                ValueLocation::Stack(offset) => {
                                    actual_stack.mark_used(offset)?;
                                }
                                _ => {}
                            }
                        }

                        debug_assert_eq!(actual_regs, ctx.regs, "{:#?}", ctx.stack);
                        debug_assert_eq!(actual_stack, ctx.allocated_stack, "{:#?}", ctx.stack);
                    }
                };
            }

            #[cfg(not(debug_assertions))]
            macro_rules! assertions {
                () => {};
            }

            assertions!();

            struct DisassemblyOpFormatter<Label>(WithLoc<Operator<Label>>);

            impl<Label> fmt::Display for DisassemblyOpFormatter<Label>
            where
                Operator<Label>: fmt::Display,
            {
                fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    match &self.0 {
                        WithLoc {
                            op: op @ Operator::Start(_),
                            ..
                        } => write!(f, "{}", op),
                        WithLoc {
                            op: op @ Operator::Declare { .. },
                            ..
                        } => write!(f, "{:5}\t{}", "", op),
                        WithLoc { op, offset } => {
                            if offset.is_default() {
                                write!(f, "{:5}\t  {}", "", op)
                            } else {
                                write!(f, "{:5}\t  {}", offset, op)
                            }
                        }
                    }
                }
            }

            op_offset_map.push((
                ctx.asm.offset(),
                Box::new(DisassemblyOpFormatter(WithLoc {
                    op: op.clone(),
                    offset,
                })),
            ));

            match op {
                Operator::Unreachable => {
                    #[cfg_attr(not(debug_assertions), allow(unused_assignments))]
                    {
                        in_block = false;
                    }

                    ctx.trap(ir::TrapCode::UnreachableCodeReached);
                }
                Operator::Start(label) => {
                    use std::collections::hash_map::Entry;

                    #[cfg_attr(not(debug_assertions), allow(unused_assignments))]
                    {
                        in_block = true;
                    }

                    if let Entry::Occupied(mut entry) = blocks.entry(BrTarget::Label(label.clone()))
                    {
                        let has_backwards_callers = {
                            let block = entry.get_mut();
                            block.is_next = false;

                            // TODO: Maybe we want to restrict Microwasm so that at least one of its callers
                            //       must be before the label. In an ideal world the restriction would be that
                            //       blocks without callers are illegal, but that's not reasonably possible
                            //       to enforce for Microwasm generated from Wasm.
                            if block.actual_num_callers.is_zero() {
                                if block.calling_convention.is_some() {
                                    return Err(error(
                                        "Block marked unreachable but has been jumped to before",
                                    ));
                                }

                                loop {
                                    let WithLoc { op: skipped, .. } = if let Some(op) = body.next()
                                    {
                                        op
                                    } else {
                                        break;
                                    }?;

                                    match skipped {
                                        Operator::End(..) | Operator::Unreachable => {
                                            #[cfg_attr(
                                                not(debug_assertions),
                                                allow(unused_assignments)
                                            )]
                                            {
                                                in_block = false;
                                            }
                                            break;
                                        }
                                        Operator::Start(..) => {
                                            return Err(error(
                                                "New block started without previous block ending",
                                            ));
                                        }
                                        Operator::Declare {
                                            label,
                                            has_backwards_callers,
                                            params,
                                            num_callers,
                                        } => {
                                            let asm_label = ctx.create_label();
                                            blocks.insert(
                                                BrTarget::Label(label),
                                                Block {
                                                    label: BrTarget::Label(asm_label),
                                                    params: params.len(),
                                                    calling_convention: None,
                                                    is_next: false,
                                                    already_emitted: false,
                                                    has_backwards_callers,
                                                    actual_num_callers: Default::default(),
                                                    num_callers,
                                                },
                                            );
                                        }
                                        _ => {}
                                    }
                                }

                                continue;
                            } else {
                                block.already_emitted = true;

                                ctx.define_label(*block.label.label().unwrap());

                                match block.calling_convention.as_ref() {
                                    Some(cc) => {
                                        ctx.set_state(cc.as_ref())?;
                                    }
                                    None => {
                                        return Err(error("No calling convention to apply"));
                                    }
                                }

                                block.has_backwards_callers
                            }
                        };

                        // To reduce memory overhead
                        if !has_backwards_callers {
                            entry.remove_entry();
                        }
                    } else {
                        return Err(error("Label defined before being declared"));
                    }
                }
                Operator::Declare {
                    label,
                    has_backwards_callers,
                    params,
                    num_callers,
                } => {
                    let asm_label = ctx.create_label();
                    blocks.insert(
                        BrTarget::Label(label),
                        Block {
                            label: BrTarget::Label(asm_label),
                            params: params.len(),
                            calling_convention: None,
                            is_next: false,
                            already_emitted: false,
                            has_backwards_callers,
                            actual_num_callers: Default::default(),
                            num_callers,
                        },
                    );
                }
                Operator::End(Targets {
                    mut targets,
                    default,
                }) => {
                    #[cfg_attr(not(debug_assertions), allow(unused_assignments))]
                    {
                        in_block = false;
                    }

                    fn cc_to_param_locs(
                        params: u32,
                        cc: &CallingConvention,
                        to_drop: Option<RangeInclusive<u32>>,
                    ) -> impl Iterator<Item = Option<ValueLocation>> + Clone + '_
                    {
                        let (start, count) = to_drop
                            .as_ref()
                            .map(|to_drop| (*to_drop.start() as usize, to_drop.clone().count()))
                            .unwrap_or_default();
                        let len = cc.arguments.locs.len();
                        let start = len.saturating_sub(start);

                        let extra = (params as usize).checked_sub(len + count).unwrap();

                        std::iter::repeat(None)
                            .take(extra)
                            .chain(cc.arguments.locs[..start].iter().cloned().map(Some))
                            .chain(std::iter::repeat(None).take(count))
                            .chain(cc.arguments.locs[start..].iter().cloned().map(Some))
                            .chain(std::iter::once(None))
                    }

                    let tmp_num_callers = |block: &Block| {
                        if block.is_next && !block.has_backwards_callers {
                            block.actual_num_callers.incremented()
                        } else {
                            block.num_callers
                        }
                    };

                    let (mut cc_and_to_drop, mut max_num_callers, params) = {
                        let def = blocks.get_mut(&default.target).unwrap();

                        (
                            def.calling_convention
                                .clone()
                                .map(|cc| (cc, default.to_drop.clone())),
                            tmp_num_callers(def),
                            def.params
                                + default
                                    .to_drop
                                    .as_ref()
                                    .map(|t| t.clone().count())
                                    .unwrap_or_default() as u32,
                        )
                    };

                    let mut adaptors = Vec::new();

                    for target in targets.iter_mut() {
                        let block = blocks.get_mut(&target.target).unwrap();
                        let mut block_to_add = None;

                        if let Some(block_cc) = &mut block.calling_convention {
                            if let Some((cc, to_drop)) = &mut cc_and_to_drop {
                                match (&cc.depth, &block_cc.depth) {
                                    (Some(a), Some(b)) if a == b => {}
                                    (_, Some(_)) if !block.already_emitted => {
                                        block_cc.depth = None;
                                    }
                                    (_, None) => {}
                                    _ => {
                                        return Err(error(
                                            "Tried to do a conditional jump with targets \
                                            that require different values for `rsp`",
                                        ));
                                    }
                                }

                                let mut block_concrete_count = 0;
                                let mut cur_concrete_count = 0;
                                let locations_equal =
                                    cc_to_param_locs(params, &block_cc, target.to_drop.clone())
                                        .zip(cc_to_param_locs(params, cc, to_drop.clone()))
                                        .all(|(block_loc, cur_loc)| {
                                            if block_loc.is_some() {
                                                block_concrete_count += 1;
                                            }

                                            if cur_loc.is_some() {
                                                cur_concrete_count += 1;
                                            }

                                            match (block_loc, cur_loc) {
                                                (Some(block_loc), Some(cur_loc)) => {
                                                    block_loc == cur_loc
                                                }
                                                _ => true,
                                            }
                                        });

                                if locations_equal {
                                    if block_concrete_count > cur_concrete_count {
                                        cc.arguments = block_cc.arguments.clone();
                                        *to_drop = target.to_drop.clone();
                                    }

                                    cc.depth = mem::take(&mut cc.depth).or(block_cc.depth.clone());
                                } else {
                                    let (new_label, next_id) = L::new_internal(internal_block_id);
                                    internal_block_id = next_id;
                                    let asm_label = ctx.create_label();
                                    block_to_add = Some((
                                        BrTarget::Label(new_label.clone()),
                                        Block {
                                            label: BrTarget::Label(asm_label),
                                            params: block.params
                                                + target
                                                    .to_drop
                                                    .clone()
                                                    .map(|t| t.count())
                                                    .unwrap_or_default()
                                                    as u32,
                                            calling_convention: None,
                                            is_next: false,
                                            already_emitted: false,
                                            has_backwards_callers: false,
                                            actual_num_callers: Default::default(),
                                            num_callers: NumCallers::One,
                                        },
                                    ));
                                    adaptors.push(Operator::Start(new_label.clone()));
                                    let current_target = mem::replace(
                                        &mut target.target,
                                        BrTarget::Label(new_label),
                                    );
                                    adaptors.push(Operator::Const(Value::I32(0)));
                                    adaptors.push(Operator::End(Targets {
                                        targets: vec![],
                                        default: current_target.into(),
                                    }));
                                }
                            } else {
                                cc_and_to_drop = Some((block_cc.clone(), target.to_drop.clone()));
                            }
                        }

                        if let Some((label, block)) = block_to_add {
                            blocks.insert(label, block);
                        }

                        let block = blocks.get_mut(&target.target).unwrap();

                        max_num_callers = max_num_callers.max(tmp_num_callers(block));

                        debug_assert_eq!(
                            params,
                            block.params
                                + target
                                    .to_drop
                                    .clone()
                                    .map(|t| t.count())
                                    .unwrap_or_default() as u32,
                            "We somehow mistracked the `to_drop` count and ended up with \
                            an incorrect param count"
                        );
                    }

                    debug_assert_le!(
                        // `params + 1` because we need 1 element for the selector.
                        params + 1,
                        ctx.stack.len() as u32,
                        "Stack has too few elements for branch"
                    );

                    if !adaptors.is_empty() {
                        debug_assert_eq!(
                            additional_instructions.len(),
                            0,
                            "We didn't emit all adaptor instructions before adding more"
                        );

                        additional_instructions = adaptors.into_iter();
                        additional_instructions_offset = offset;
                    }

                    let (cc, selector) = match cc_and_to_drop {
                        Some((cc, to_drop)) => {
                            let should_serialize = if max_num_callers.is_zero() {
                                MaybeCCLoc::Serialize
                            } else {
                                MaybeCCLoc::NoSerialize
                            };

                            // TODO: We can also use `NoSerialize` for any elements that are only
                            //       serialized for blocks where `!block.should_serialize_args()`.
                            let locs = cc_to_param_locs(params, &cc, to_drop.clone())
                                .map(|loc| {
                                    loc.and_then(|loc| CCLoc::try_from(loc).ok())
                                        .map(MaybeCCLoc::Concrete)
                                        .unwrap_or(should_serialize)
                                })
                                .collect::<Vec<_>>();

                            let mut tmp = ctx.serialize_block_args(locs, cc.depth)?;

                            let selector = tmp.arguments.locs.pop().unwrap();

                            (tmp, selector)
                        }
                        None => {
                            if max_num_callers.is_many() {
                                let mut tmp = ctx.serialize_block_args(
                                    std::iter::repeat(MaybeCCLoc::Serialize)
                                        .take(params as usize)
                                        .chain(std::iter::once(MaybeCCLoc::NoSerialize))
                                        .collect::<Vec<_>>()
                                        .into_iter(),
                                    None,
                                )?;

                                tmp.depth = Some(tmp.arguments.max_depth.clone());

                                let selector = tmp.arguments.locs.pop().unwrap();

                                (tmp, selector)
                            } else {
                                let selector = ctx.pop()?;
                                (ctx.virtual_calling_convention(), selector)
                            }
                        }
                    };

                    let mut target_set = std::collections::HashSet::new();

                    for target in targets.iter().chain(std::iter::once(&default)) {
                        let block = blocks.get_mut(&target.target).unwrap();

                        if let Some(block_calling_convention) = &mut block.calling_convention {
                            debug_assert_eq!(
                                block_calling_convention.arguments.locs,
                                {
                                    let mut cc = cc.clone();

                                    if let Some(to_drop) = target.to_drop.clone() {
                                        drop_elements(&mut cc.arguments.locs, to_drop);
                                    }

                                    debug_assert_eq!(
                                        cc.arguments.locs.len(),
                                        block.params as usize
                                    );

                                    cc.arguments.locs
                                },
                                "Calling convention doesn't match"
                            );

                            match (&cc.depth, &block_calling_convention.depth) {
                                (Some(a), Some(b)) if a == b => {}
                                (_, None) => {}
                                _ => {
                                    return Err(error(
                                        "Programmer error: Tried to conditionally jump to targets \
                                        that require different values for `rsp`",
                                    ));
                                }
                            }
                        } else {
                            let mut cc = cc.clone();
                            if let Some(to_drop) = target.to_drop.clone() {
                                drop_elements(&mut cc.arguments.locs, to_drop);
                            }

                            debug_assert_eq!(cc.arguments.locs.len(), block.params as usize,);

                            // TODO: Take `target_set` into account
                            let num_callers = tmp_num_callers(block);

                            if num_callers.is_many() {
                                cc.depth = None;
                            }

                            block.calling_convention = Some(cc);
                        }

                        target_set.insert(&target.target);
                    }

                    for target in target_set {
                        blocks.get_mut(&target).unwrap().actual_num_callers.inc();
                    }

                    let depth = cc.depth.clone();

                    let default_label = {
                        let block = &blocks[&default.target];
                        Target {
                            target: block.label,
                            action: if block.is_next {
                                BrAction::Continue
                            } else {
                                BrAction::Jump
                            },
                        }
                    };

                    ctx.end_block(
                        targets.iter().map(|t| {
                            let block = &blocks[&t.target];
                            Target {
                                target: block.label,
                                action: if block.is_next {
                                    BrAction::Continue
                                } else {
                                    BrAction::Jump
                                },
                            }
                        }),
                        default_label,
                        depth,
                        selector,
                    )?;
                }
                Operator::Swap(depth) => ctx.swap(depth)?,
                Operator::Pick(depth) => ctx.pick(depth)?,
                Operator::Eq(I32) => ctx.i32_eq()?,
                Operator::Eqz(Size::_32) => ctx.i32_eqz()?,
                Operator::Ne(I32) => ctx.i32_neq()?,
                Operator::Lt(SI32) => ctx.i32_lt_s()?,
                Operator::Le(SI32) => ctx.i32_le_s()?,
                Operator::Gt(SI32) => ctx.i32_gt_s()?,
                Operator::Ge(SI32) => ctx.i32_ge_s()?,
                Operator::Lt(SU32) => ctx.i32_lt_u()?,
                Operator::Le(SU32) => ctx.i32_le_u()?,
                Operator::Gt(SU32) => ctx.i32_gt_u()?,
                Operator::Ge(SU32) => ctx.i32_ge_u()?,
                Operator::Add(I32) => ctx.i32_add()?,
                Operator::Sub(I32) => ctx.i32_sub()?,
                Operator::And(Size::_32) => ctx.i32_and()?,
                Operator::Or(Size::_32) => ctx.i32_or()?,
                Operator::Xor(Size::_32) => ctx.i32_xor()?,
                Operator::Mul(I32) => ctx.i32_mul()?,
                Operator::Div(SU32) => ctx.i32_div_u()?,
                Operator::Div(SI32) => ctx.i32_div_s()?,
                Operator::Rem(sint::I32) => ctx.i32_rem_s()?,
                Operator::Rem(sint::U32) => ctx.i32_rem_u()?,
                Operator::Shl(Size::_32) => ctx.i32_shl()?,
                Operator::Shr(sint::I32) => ctx.i32_shr_s()?,
                Operator::Shr(sint::U32) => ctx.i32_shr_u()?,
                Operator::Rotl(Size::_32) => ctx.i32_rotl()?,
                Operator::Rotr(Size::_32) => ctx.i32_rotr()?,
                Operator::Clz(Size::_32) => ctx.i32_clz()?,
                Operator::Ctz(Size::_32) => ctx.i32_ctz()?,
                Operator::Popcnt(Size::_32) => ctx.i32_popcnt()?,
                Operator::Eq(I64) => ctx.i64_eq()?,
                Operator::Eqz(Size::_64) => ctx.i64_eqz()?,
                Operator::Ne(I64) => ctx.i64_neq()?,
                Operator::Lt(SI64) => ctx.i64_lt_s()?,
                Operator::Le(SI64) => ctx.i64_le_s()?,
                Operator::Gt(SI64) => ctx.i64_gt_s()?,
                Operator::Ge(SI64) => ctx.i64_ge_s()?,
                Operator::Lt(SU64) => ctx.i64_lt_u()?,
                Operator::Le(SU64) => ctx.i64_le_u()?,
                Operator::Gt(SU64) => ctx.i64_gt_u()?,
                Operator::Ge(SU64) => ctx.i64_ge_u()?,
                Operator::Add(I64) => ctx.i64_add()?,
                Operator::Sub(I64) => ctx.i64_sub()?,
                Operator::And(Size::_64) => ctx.i64_and()?,
                Operator::Or(Size::_64) => ctx.i64_or()?,
                Operator::Xor(Size::_64) => ctx.i64_xor()?,
                Operator::Mul(I64) => ctx.i64_mul()?,
                Operator::Div(SU64) => ctx.i64_div_u()?,
                Operator::Div(SI64) => ctx.i64_div_s()?,
                Operator::Rem(sint::I64) => ctx.i64_rem_s()?,
                Operator::Rem(sint::U64) => ctx.i64_rem_u()?,
                Operator::Shl(Size::_64) => ctx.i64_shl()?,
                Operator::Shr(sint::I64) => ctx.i64_shr_s()?,
                Operator::Shr(sint::U64) => ctx.i64_shr_u()?,
                Operator::Rotl(Size::_64) => ctx.i64_rotl()?,
                Operator::Rotr(Size::_64) => ctx.i64_rotr()?,
                Operator::Clz(Size::_64) => ctx.i64_clz()?,
                Operator::Ctz(Size::_64) => ctx.i64_ctz()?,
                Operator::Popcnt(Size::_64) => ctx.i64_popcnt()?,
                Operator::Add(F32) => ctx.f32_add()?,
                Operator::Mul(F32) => ctx.f32_mul()?,
                Operator::Sub(F32) => ctx.f32_sub()?,
                Operator::Div(SF32) => ctx.f32_div()?,
                Operator::Min(Size::_32) => ctx.f32_min()?,
                Operator::Max(Size::_32) => ctx.f32_max()?,
                Operator::Copysign(Size::_32) => ctx.f32_copysign()?,
                Operator::Sqrt(Size::_32) => ctx.f32_sqrt()?,
                Operator::Neg(Size::_32) => ctx.f32_neg()?,
                Operator::Abs(Size::_32) => ctx.f32_abs()?,
                Operator::Floor(Size::_32) => ctx.f32_floor()?,
                Operator::Ceil(Size::_32) => ctx.f32_ceil()?,
                Operator::Nearest(Size::_32) => ctx.f32_nearest()?,
                Operator::Trunc(Size::_32) => ctx.f32_trunc()?,
                Operator::Eq(F32) => ctx.f32_eq()?,
                Operator::Ne(F32) => ctx.f32_ne()?,
                Operator::Gt(SF32) => ctx.f32_gt()?,
                Operator::Ge(SF32) => ctx.f32_ge()?,
                Operator::Lt(SF32) => ctx.f32_lt()?,
                Operator::Le(SF32) => ctx.f32_le()?,
                Operator::Add(F64) => ctx.f64_add()?,
                Operator::Mul(F64) => ctx.f64_mul()?,
                Operator::Sub(F64) => ctx.f64_sub()?,
                Operator::Div(SF64) => ctx.f64_div()?,
                Operator::Min(Size::_64) => ctx.f64_min()?,
                Operator::Max(Size::_64) => ctx.f64_max()?,
                Operator::Copysign(Size::_64) => ctx.f64_copysign()?,
                Operator::Sqrt(Size::_64) => ctx.f64_sqrt()?,
                Operator::Neg(Size::_64) => ctx.f64_neg()?,
                Operator::Abs(Size::_64) => ctx.f64_abs()?,
                Operator::Floor(Size::_64) => ctx.f64_floor()?,
                Operator::Ceil(Size::_64) => ctx.f64_ceil()?,
                Operator::Nearest(Size::_64) => ctx.f64_nearest()?,
                Operator::Trunc(Size::_64) => ctx.f64_trunc()?,
                Operator::Eq(F64) => ctx.f64_eq()?,
                Operator::Ne(F64) => ctx.f64_ne()?,
                Operator::Gt(SF64) => ctx.f64_gt()?,
                Operator::Ge(SF64) => ctx.f64_ge()?,
                Operator::Lt(SF64) => ctx.f64_lt()?,
                Operator::Le(SF64) => ctx.f64_le()?,
                Operator::Drop(range) => ctx.drop(range)?,
                Operator::Const(val) => ctx.const_(val)?,
                Operator::I32WrapFromI64 => ctx.i32_wrap_from_i64()?,
                Operator::I32ReinterpretFromF32 => ctx.i32_reinterpret_from_f32()?,
                Operator::I64ReinterpretFromF64 => ctx.i64_reinterpret_from_f64()?,
                Operator::F32ReinterpretFromI32 => ctx.f32_reinterpret_from_i32()?,
                Operator::F64ReinterpretFromI64 => ctx.f64_reinterpret_from_i64()?,
                Operator::ITruncFromF {
                    input_ty: Size::_32,
                    output_ty: sint::I32,
                } => {
                    ctx.i32_truncate_f32_s()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_32,
                    output_ty: sint::U32,
                } => {
                    ctx.i32_truncate_f32_u()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_64,
                    output_ty: sint::I32,
                } => {
                    ctx.i32_truncate_f64_s()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_64,
                    output_ty: sint::U32,
                } => {
                    ctx.i32_truncate_f64_u()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_32,
                    output_ty: sint::I64,
                } => {
                    ctx.i64_truncate_f32_s()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_32,
                    output_ty: sint::U64,
                } => {
                    ctx.i64_truncate_f32_u()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_64,
                    output_ty: sint::I64,
                } => {
                    ctx.i64_truncate_f64_s()?;
                }
                Operator::ITruncFromF {
                    input_ty: Size::_64,
                    output_ty: sint::U64,
                } => {
                    ctx.i64_truncate_f64_u()?;
                }
                Operator::Extend8 { size: Size::_32 } => ctx.i32_convert_from_i8()?,
                Operator::Extend16 { size: Size::_32 } => ctx.i32_convert_from_i16()?,
                Operator::Extend8 { size: Size::_64 } => ctx.i64_convert_from_i8()?,
                Operator::Extend16 { size: Size::_64 } => ctx.i64_convert_from_i16()?,
                Operator::Extend32 {
                    sign: Signedness::Unsigned,
                } => ctx.u64_convert_from_u32()?,
                Operator::Extend32 {
                    sign: Signedness::Signed,
                } => ctx.i64_convert_from_i32()?,
                Operator::FConvertFromI {
                    input_ty: sint::I32,
                    output_ty: Size::_32,
                } => ctx.f32_convert_from_i32_s()?,
                Operator::FConvertFromI {
                    input_ty: sint::I32,
                    output_ty: Size::_64,
                } => ctx.f64_convert_from_i32_s()?,
                Operator::FConvertFromI {
                    input_ty: sint::I64,
                    output_ty: Size::_32,
                } => ctx.f32_convert_from_i64_s()?,
                Operator::FConvertFromI {
                    input_ty: sint::I64,
                    output_ty: Size::_64,
                } => ctx.f64_convert_from_i64_s()?,
                Operator::FConvertFromI {
                    input_ty: sint::U32,
                    output_ty: Size::_32,
                } => ctx.f32_convert_from_i32_u()?,
                Operator::FConvertFromI {
                    input_ty: sint::U32,
                    output_ty: Size::_64,
                } => ctx.f64_convert_from_i32_u()?,
                Operator::FConvertFromI {
                    input_ty: sint::U64,
                    output_ty: Size::_32,
                } => ctx.f32_convert_from_i64_u()?,
                Operator::FConvertFromI {
                    input_ty: sint::U64,
                    output_ty: Size::_64,
                } => ctx.f64_convert_from_i64_u()?,
                Operator::F64PromoteFromF32 => ctx.f64_from_f32()?,
                Operator::F32DemoteFromF64 => ctx.f32_from_f64()?,
                Operator::Load8 {
                    ty: sint::U32,
                    memarg,
                } => ctx.i32_load8_u(memarg.offset)?,
                Operator::Load16 {
                    ty: sint::U32,
                    memarg,
                } => ctx.i32_load16_u(memarg.offset)?,
                Operator::Load8 {
                    ty: sint::I32,
                    memarg,
                } => ctx.i32_load8_s(memarg.offset)?,
                Operator::Load16 {
                    ty: sint::I32,
                    memarg,
                } => ctx.i32_load16_s(memarg.offset)?,
                Operator::Load8 {
                    ty: sint::U64,
                    memarg,
                } => ctx.i64_load8_u(memarg.offset)?,
                Operator::Load16 {
                    ty: sint::U64,
                    memarg,
                } => ctx.i64_load16_u(memarg.offset)?,
                Operator::Load8 {
                    ty: sint::I64,
                    memarg,
                } => ctx.i64_load8_s(memarg.offset)?,
                Operator::Load16 {
                    ty: sint::I64,
                    memarg,
                } => ctx.i64_load16_s(memarg.offset)?,
                Operator::Load32 {
                    sign: Signedness::Unsigned,
                    memarg,
                } => ctx.i64_load32_u(memarg.offset)?,
                Operator::Load32 {
                    sign: Signedness::Signed,
                    memarg,
                } => ctx.i64_load32_s(memarg.offset)?,
                Operator::Load { ty: I32, memarg } => ctx.i32_load(memarg.offset)?,
                Operator::Load { ty: F32, memarg } => ctx.f32_load(memarg.offset)?,
                Operator::Load { ty: I64, memarg } => ctx.i64_load(memarg.offset)?,
                Operator::Load { ty: F64, memarg } => ctx.f64_load(memarg.offset)?,
                Operator::Store8 { memarg, .. } => ctx.store8(memarg.offset)?,
                Operator::Store16 { memarg, .. } => ctx.store16(memarg.offset)?,
                Operator::Store32 { memarg }
                | Operator::Store { ty: I32, memarg }
                | Operator::Store { ty: F32, memarg } => ctx.store32(memarg.offset)?,
                Operator::Store { ty: I64, memarg } | Operator::Store { ty: F64, memarg } => {
                    ctx.store64(memarg.offset)?
                }
                Operator::GlobalGet(idx) => ctx.get_global(idx)?,
                Operator::GlobalSet(idx) => ctx.set_global(idx)?,
                Operator::Select => {
                    ctx.select()?;
                }
                Operator::MemorySize { .. } => {
                    ctx.memory_size()?;
                }
                Operator::MemoryGrow { .. } => {
                    ctx.memory_grow()?;
                }
                Operator::Call { function_index } => {
                    let callee_ty = module_context.func_type(function_index);

                    if let Some(defined_index) = module_context.defined_func_index(function_index) {
                        if defined_index == func_idx {
                            ctx.call_direct_self(
                                callee_ty.params().iter().map(|t| t.to_microwasm_type()),
                                callee_ty.returns().iter().map(|t| t.to_microwasm_type()),
                            )?;
                        } else {
                            ctx.call_direct(
                                function_index,
                                callee_ty.params().iter().map(|t| t.to_microwasm_type()),
                                callee_ty.returns().iter().map(|t| t.to_microwasm_type()),
                            )?;
                        }
                    } else {
                        ctx.call_direct_imported(
                            function_index,
                            callee_ty.params().iter().map(|t| t.to_microwasm_type()),
                            callee_ty.returns().iter().map(|t| t.to_microwasm_type()),
                        )?;
                    }
                }
                Operator::CallIndirect {
                    type_index,
                    table_index,
                } => {
                    if table_index != 0 {
                        return Err(error("table_index not equal to 0"));
                    }

                    let callee_ty = module_context.signature(type_index);

                    ctx.call_indirect(
                        type_index,
                        callee_ty.params().iter().map(|t| t.to_microwasm_type()),
                        callee_ty.returns().iter().map(|t| t.to_microwasm_type()),
                    )?;
                }
            }
        }

        ctx.epilogue();
    }

    session.op_offset_map = op_offset_map;

    Ok(())
}
