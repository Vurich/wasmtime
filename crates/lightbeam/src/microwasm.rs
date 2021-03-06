use crate::{
    error::{error, Error},
    module::{ModuleContext, SigType, Signature},
};
use cranelift_codegen::ir::SourceLoc;
use itertools::Either;
use std::{
    convert::TryInto,
    fmt,
    iter::{self, FromIterator},
    ops::RangeInclusive,
};
use wasmparser::{
    FunctionBody, Ieee32 as WasmIeee32, Ieee64 as WasmIeee64,
    MemoryImmediate as WasmMemoryImmediate, Operator as WasmOperator, OperatorsReader,
};

pub fn dis<L>(
    mut out: impl std::io::Write,
    function_name: impl fmt::Display,
    microwasm: impl IntoIterator<Item = Operator<L>>,
) -> std::io::Result<()>
where
    BrTarget<L>: fmt::Display,
    L: Clone,
{
    writeln!(out, ".fn_{}:", function_name)?;

    let p = "      ";
    for op in microwasm {
        if op.is_label() || op.is_block() {
            writeln!(out, "{}", op)?;
        } else {
            writeln!(out, "{}{}", p, op)?;
        }
    }

    Ok(())
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Ieee32(u32);
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct Ieee64(u64);

impl Ieee32 {
    pub fn to_bits(self) -> u32 {
        self.0
    }

    pub fn from_bits(other: u32) -> Self {
        Ieee32(other)
    }
}

impl From<WasmIeee32> for Ieee32 {
    fn from(other: WasmIeee32) -> Self {
        Self::from_bits(other.bits())
    }
}

impl Ieee64 {
    pub fn to_bits(self) -> u64 {
        self.0
    }

    pub fn from_bits(other: u64) -> Self {
        Ieee64(other)
    }
}

impl From<WasmIeee64> for Ieee64 {
    fn from(other: WasmIeee64) -> Self {
        Self::from_bits(other.bits())
    }
}

/// A constant value embedded in the instructions
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Value {
    I32(i32),
    I64(i64),
    F32(Ieee32),
    F64(Ieee64),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::I32(v) => write!(f, "{}i32", v),
            Value::I64(v) => write!(f, "{}i64", v),
            Value::F32(v) => write!(f, "{}f32", f32::from_bits(v.0)),
            Value::F64(v) => write!(f, "{}f64", f64::from_bits(v.0)),
        }
    }
}

impl Value {
    pub fn as_int(self) -> Option<i64> {
        self.as_i64().or_else(|| self.as_i32().map(|i| i as _))
    }

    pub fn as_bytes(self) -> i64 {
        match self {
            Value::I32(val) => val as _,
            Value::I64(val) => val,
            Value::F32(val) => val.0 as _,
            Value::F64(val) => val.0 as _,
        }
    }

    pub fn as_i32(self) -> Option<i32> {
        match self {
            Value::I32(val) => Some(val),
            _ => None,
        }
    }

    pub fn as_i64(self) -> Option<i64> {
        match self {
            Value::I64(val) => Some(val),
            _ => None,
        }
    }

    pub fn as_f32(self) -> Option<Ieee32> {
        match self {
            Value::F32(val) => Some(val),
            _ => None,
        }
    }

    pub fn as_f64(self) -> Option<Ieee64> {
        match self {
            Value::F64(val) => Some(val),
            _ => None,
        }
    }

    pub fn type_(&self) -> SignlessType {
        match self {
            Value::I32(_) => Type::Int(Size::_32),
            Value::I64(_) => Type::Int(Size::_64),
            Value::F32(Ieee32(_)) => Type::Float(Size::_32),
            Value::F64(Ieee64(_)) => Type::Float(Size::_64),
        }
    }

    fn default_for_type(ty: SignlessType) -> Self {
        match ty {
            Type::Int(Size::_32) => Value::I32(0),
            Type::Int(Size::_64) => Value::I64(0),
            Type::Float(Size::_32) => Value::F32(Ieee32(0)),
            Type::Float(Size::_64) => Value::F64(Ieee64(0)),
        }
    }
}

impl From<i32> for Value {
    fn from(other: i32) -> Self {
        Value::I32(other)
    }
}
impl From<i64> for Value {
    fn from(other: i64) -> Self {
        Value::I64(other)
    }
}
impl From<u32> for Value {
    fn from(other: u32) -> Self {
        Value::I32(other as _)
    }
}
impl From<u64> for Value {
    fn from(other: u64) -> Self {
        Value::I64(other as _)
    }
}
impl From<Ieee32> for Value {
    fn from(other: Ieee32) -> Self {
        Value::F32(other)
    }
}
impl From<Ieee64> for Value {
    fn from(other: Ieee64) -> Self {
        Value::F64(other)
    }
}

/// Whether to interpret an integer as signed or unsigned
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Signedness {
    Signed,
    Unsigned,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Size {
    _32,
    _64,
}

type Int = Size;
type Float = Size;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SignfulInt(pub Signedness, pub Size);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Type<I> {
    Int(I),
    Float(Size),
}

pub trait IntoType<T> {
    fn into_type() -> T;
}

impl IntoType<SignlessType> for i32 {
    fn into_type() -> SignlessType {
        I32
    }
}

impl IntoType<SignlessType> for i64 {
    fn into_type() -> SignlessType {
        I64
    }
}

impl IntoType<SignlessType> for u32 {
    fn into_type() -> SignlessType {
        I32
    }
}

impl IntoType<SignlessType> for u64 {
    fn into_type() -> SignlessType {
        I64
    }
}

impl IntoType<SignlessType> for f32 {
    fn into_type() -> SignlessType {
        F32
    }
}

impl IntoType<SignlessType> for f64 {
    fn into_type() -> SignlessType {
        F64
    }
}

impl<I> Type<I> {
    pub fn for_<T: IntoType<Self>>() -> Self {
        T::into_type()
    }
}

impl fmt::Display for SignfulType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Type::Int(i) => write!(f, "{}", i),
            Type::Float(Size::_32) => write!(f, "f32"),
            Type::Float(Size::_64) => write!(f, "f64"),
        }
    }
}

impl fmt::Display for SignlessType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Type::Int(Size::_32) => write!(f, "i32"),
            Type::Int(Size::_64) => write!(f, "i64"),
            Type::Float(Size::_32) => write!(f, "f32"),
            Type::Float(Size::_64) => write!(f, "f64"),
        }
    }
}

impl fmt::Display for SignfulInt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SignfulInt(Signedness::Signed, Size::_32) => write!(f, "i32"),
            SignfulInt(Signedness::Unsigned, Size::_32) => write!(f, "u32"),
            SignfulInt(Signedness::Signed, Size::_64) => write!(f, "i64"),
            SignfulInt(Signedness::Unsigned, Size::_64) => write!(f, "u64"),
        }
    }
}

pub type SignlessType = Type<Size>;
pub type SignfulType = Type<SignfulInt>;

pub const I32: SignlessType = Type::Int(Size::_32);
pub const I64: SignlessType = Type::Int(Size::_64);
pub const F32: SignlessType = Type::Float(Size::_32);
pub const F64: SignlessType = Type::Float(Size::_64);

pub mod sint {
    use super::{Signedness, SignfulInt, Size};

    pub const I32: SignfulInt = SignfulInt(Signedness::Signed, Size::_32);
    pub const I64: SignfulInt = SignfulInt(Signedness::Signed, Size::_64);
    pub const U32: SignfulInt = SignfulInt(Signedness::Unsigned, Size::_32);
    pub const U64: SignfulInt = SignfulInt(Signedness::Unsigned, Size::_64);
}

pub const SI32: SignfulType = Type::Int(sint::I32);
pub const SI64: SignfulType = Type::Int(sint::I64);
pub const SU32: SignfulType = Type::Int(sint::U32);
pub const SU64: SignfulType = Type::Int(sint::U64);
pub const SF32: SignfulType = Type::Float(Size::_32);
pub const SF64: SignfulType = Type::Float(Size::_64);

impl SignlessType {
    pub fn from_wasm_block(other: wasmparser::Type) -> Result<Option<Self>, Error> {
        use wasmparser::Type;

        match other {
            Type::I32 => Ok(Some(I32)),
            Type::I64 => Ok(Some(I64)),
            Type::F32 => Ok(Some(F32)),
            Type::F64 => Ok(Some(F64)),
            Type::EmptyBlockType => Ok(None),
            _ => Err(Error::Input("Invalid type".into())),
        }
    }

    pub fn from_wasm(other: wasmparser::Type) -> Result<Self, Error> {
        match Self::from_wasm_block(other) {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err(Error::Input("Invalid type".into())),
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Targets<L> {
    pub targets: Vec<BrTargetDrop<L>>,
    pub default: BrTargetDrop<L>,
}

impl<L> From<BrTargetDrop<L>> for Targets<L> {
    fn from(other: BrTargetDrop<L>) -> Self {
        Self {
            targets: Default::default(),
            default: other,
        }
    }
}

impl<L> From<BrTarget<L>> for Targets<L> {
    fn from(other: BrTarget<L>) -> Self {
        Self {
            targets: Default::default(),
            default: other.into(),
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum NameTag {
    Header,
    Else,
    End,
    Internal,
}

pub type WasmLabel = (u32, NameTag);

impl crate::function_body::MakeInternalLabel for WasmLabel {
    type Id = u32;

    const FIRST_ID: Self::Id = 0;

    fn new_internal(id: Self::Id) -> (Self, Self::Id) {
        ((id, NameTag::Internal), id.checked_add(1).unwrap())
    }
}

pub type OperatorFromWasm = Operator<WasmLabel>;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum BrTarget<L> {
    Return,
    Label(L),
}

impl<L> BrTarget<L> {
    pub fn label(&self) -> Option<&L> {
        match self {
            BrTarget::Return => None,
            BrTarget::Label(l) => Some(l),
        }
    }
}

impl<L> From<L> for BrTarget<L> {
    fn from(other: L) -> Self {
        BrTarget::Label(other)
    }
}

impl fmt::Display for BrTarget<WasmLabel> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BrTarget::Return => write!(f, ".return"),
            BrTarget::Label((i, NameTag::Header)) => write!(f, ".L{}", i),
            BrTarget::Label((i, NameTag::Else)) => write!(f, ".L{}_else", i),
            BrTarget::Label((i, NameTag::End)) => write!(f, ".L{}_end", i),
            BrTarget::Label((i, NameTag::Internal)) => write!(f, ".INTERNAL{}", i),
        }
    }
}

impl fmt::Display for BrTarget<&str> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BrTarget::Return => write!(f, ".return"),
            BrTarget::Label(l) => write!(f, ".L{}", l),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct BrTargetDrop<L> {
    pub target: BrTarget<L>,
    pub to_drop: Option<RangeInclusive<u32>>,
}

impl<L> From<BrTarget<L>> for BrTargetDrop<L> {
    fn from(other: BrTarget<L>) -> Self {
        BrTargetDrop {
            target: other,
            to_drop: None,
        }
    }
}

impl<L> fmt::Display for BrTargetDrop<L>
where
    BrTarget<L>: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(drop) = &self.to_drop {
            write!(
                f,
                "({}, drop {}..={})",
                self.target,
                drop.start(),
                drop.end()
            )
        } else {
            write!(f, "{}", self.target)
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct MemoryImmediate {
    pub flags: u32,
    pub offset: u32,
}

impl From<WasmMemoryImmediate> for MemoryImmediate {
    fn from(other: WasmMemoryImmediate) -> Self {
        MemoryImmediate {
            flags: other.flags,
            offset: other.offset,
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    inner: Vec<SignlessType>,
}

#[cfg(not(debug_assertions))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    inner: u32,
}

#[cfg(debug_assertions)]
impl Params {
    pub fn new<I: ExactSizeIterator<Item = SignlessType>>(iter: I) -> Self {
        Params {
            inner: iter.collect(),
        }
    }

    pub fn len(&self) -> u32 {
        self.inner.len() as u32
    }
}

#[cfg(not(debug_assertions))]
impl Params {
    pub fn new<I: ExactSizeIterator<Item = SignlessType>>(iter: I) -> Self {
        Params {
            inner: iter.len() as u32,
        }
    }

    pub fn len(&self) -> u32 {
        self.inner
    }
}

#[cfg(debug_assertions)]
impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut iter = self.inner.iter();
        write!(f, "[")?;
        if let Some(p) = iter.next() {
            write!(f, "{}", p)?;
            for p in iter {
                write!(f, ", {}", p)?;
            }
        }
        write!(f, "]")?;

        Ok(())
    }
}

#[cfg(not(debug_assertions))]
impl fmt::Display for Params {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut iter = 0..self.inner;
        write!(f, "[")?;
        if let Some(_) = iter.next() {
            write!(f, "??")?;
            for _ in iter {
                write!(f, ", ??")?;
            }
        }
        write!(f, "]")?;

        Ok(())
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Hash, Debug)]
pub enum NumCallers {
    Zero,
    One,
    Many,
}

impl fmt::Display for NumCallers {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            NumCallers::Zero => write!(f, "0"),
            NumCallers::One => write!(f, "1"),
            NumCallers::Many => write!(f, "??"),
        }
    }
}

impl Default for NumCallers {
    fn default() -> Self {
        NumCallers::Zero
    }
}

impl NumCallers {
    pub fn is_zero(&self) -> bool {
        *self == NumCallers::Zero
    }

    pub fn is_many(&self) -> bool {
        *self == NumCallers::Many
    }

    pub fn inc(&mut self) {
        *self = match *self {
            NumCallers::Zero => NumCallers::One,
            NumCallers::One | NumCallers::Many => NumCallers::Many,
        }
    }

    pub fn incremented(&self) -> Self {
        let mut out = *self;
        out.inc();
        out
    }
}

// TODO: Explicit VmCtx?
#[derive(Debug, Clone)]
pub enum Operator<Label> {
    /// Explicit trap instruction
    Unreachable,
    /// Define metadata for a block - its label, its signature, whether it has backwards callers etc. It
    /// is an error to branch to a block that has yet to be defined.
    Declare {
        label: Label,
        params: Params,
        // TODO: Ideally we'd have `num_backwards_callers` but we can't know that for WebAssembly
        has_backwards_callers: bool,
        num_callers: NumCallers,
    },
    /// Start a new block. It is an error if the previous block has not been closed by emitting `End`
    Start(Label),
    /// Pop a value off the top of the stack, jump to `table[value.min(table.len() - 1)]`. All elements
    /// in the table must have the same parameters.
    End(
        /// The table of labels to jump to - the index should be clamped to the length of the table
        Targets<Label>,
    ),
    /// Call a function
    Call {
        function_index: u32,
    },
    /// Pop an `i32` off the top of the stack, index into the table at `table_index` and call that function
    CallIndirect {
        type_index: u32,
        table_index: u32,
    },
    /// Pop an element off of the stack and discard it.
    Drop(RangeInclusive<u32>),
    /// Pop an `i32` off of the stack and 2 elements off of the stack, call them `A` and `B` where `A` is the
    /// first element popped and `B` is the second. If the `i32` is 0 then discard `B` and push `A` back onto
    /// the stack, otherwise discard `A` and push `B` back onto the stack.
    Select,
    /// Duplicate the element at depth `depth` to the top of the stack. This can be used to implement
    /// `LocalGet`.
    Pick(u32),
    /// Swap the top element of the stack with the element at depth `depth`. This can be used to implement
    /// `LocalSet`.
    // TODO: Is it better to have `Swap`, to have `Pull` (which moves the `nth` element instead of swapping)
    //       or to have both?
    Swap(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    Load {
        ty: SignlessType,
        memarg: MemoryImmediate,
    },
    Load8 {
        ty: SignfulInt,
        memarg: MemoryImmediate,
    },
    Load16 {
        ty: SignfulInt,
        memarg: MemoryImmediate,
    },
    // Only available for {I,U}64
    // TODO: Roll this into `Load` somehow?
    Load32 {
        sign: Signedness,
        memarg: MemoryImmediate,
    },
    Store {
        ty: SignlessType,
        memarg: MemoryImmediate,
    },
    Store8 {
        /// `ty` on integers
        ty: Int,
        memarg: MemoryImmediate,
    },
    Store16 {
        /// `ty` on integers
        ty: Int,
        memarg: MemoryImmediate,
    },
    // Only available for I64
    // TODO: Roll this into `Store` somehow?
    Store32 {
        memarg: MemoryImmediate,
    },
    MemorySize {
        reserved: u32,
    },
    MemoryGrow {
        reserved: u32,
    },
    Const(Value),
    Eq(SignlessType),
    Ne(SignlessType),
    /// `eqz` on integers
    Eqz(Int),
    Lt(SignfulType),
    Gt(SignfulType),
    Le(SignfulType),
    Ge(SignfulType),
    Add(SignlessType),
    Sub(SignlessType),
    Mul(SignlessType),
    /// `clz` on integers
    Clz(Int),
    /// `ctz` on integers
    Ctz(Int),
    /// `popcnt` on integers
    Popcnt(Int),
    Div(SignfulType),
    Rem(SignfulInt),
    And(Int),
    Or(Int),
    Xor(Int),
    Shl(Int),
    Shr(SignfulInt),
    Rotl(Int),
    Rotr(Int),
    Abs(Float),
    Neg(Float),
    Ceil(Float),
    Floor(Float),
    Trunc(Float),
    Nearest(Float),
    Sqrt(Float),
    Min(Float),
    Max(Float),
    Copysign(Float),
    I32WrapFromI64,
    ITruncFromF {
        input_ty: Float,
        output_ty: SignfulInt,
    },
    FConvertFromI {
        input_ty: SignfulInt,
        output_ty: Float,
    },
    F32DemoteFromF64,
    F64PromoteFromF32,
    I32ReinterpretFromF32,
    I64ReinterpretFromF64,
    F32ReinterpretFromI32,
    F64ReinterpretFromI64,
    Extend8 {
        size: Size,
    },
    Extend16 {
        size: Size,
    },
    Extend32 {
        sign: Signedness,
    },
}

impl<L> Operator<L> {
    pub fn is_label(&self) -> bool {
        match self {
            Operator::Start(..) => true,
            _ => false,
        }
    }

    pub fn is_block(&self) -> bool {
        match self {
            Operator::Declare { .. } => true,
            _ => false,
        }
    }

    pub fn end_wasm_block(params: Params, label: L) -> Self {
        Operator::Declare {
            params,
            label,
            has_backwards_callers: false,
            // TODO
            num_callers: NumCallers::Many,
        }
    }

    pub fn block(params: Params, label: L) -> Self {
        Operator::Declare {
            params,
            label,
            has_backwards_callers: false,
            num_callers: NumCallers::One,
        }
    }

    pub fn loop_(params: Params, label: L) -> Self {
        Operator::Declare {
            params,
            label,
            has_backwards_callers: true,
            num_callers: NumCallers::Many,
        }
    }
}

impl<L> fmt::Display for Operator<L>
where
    BrTarget<L>: fmt::Display,
    L: Clone,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Operator::Unreachable => write!(f, "unreachable"),
            Operator::Start(label) => write!(f, "start {}:", BrTarget::Label(label.clone())),
            Operator::Declare {
                label,
                params,
                has_backwards_callers,
                num_callers,
            } => {
                write!(f, "def {} :: {}", BrTarget::Label(label.clone()), params)?;

                if *has_backwards_callers {
                    write!(f, " has_backwards_callers")?;
                }

                write!(f, " num_callers={}", num_callers)?;

                Ok(())
            }
            Operator::End(Targets { targets, default }) => {
                write!(f, "end ")?;
                let mut iter = targets.iter();
                if let Some(p) = iter.next() {
                    write!(f, "[{}", p)?;
                    for p in iter {
                        write!(f, ", {}", p)?;
                    }
                    write!(f, "], ")?;
                }

                write!(f, "{}", default)
            }
            Operator::Call { function_index } => write!(f, "call {}", function_index),
            Operator::CallIndirect { .. } => write!(f, "call_indirect"),
            Operator::Drop(range) => {
                write!(f, "drop")?;

                match range.clone().into_inner() {
                    (0, 0) => {}
                    (start, end) if start == end => {
                        write!(f, " {}", start)?;
                    }
                    (start, end) => {
                        write!(f, " {}..={}", start, end)?;
                    }
                }

                Ok(())
            }
            Operator::Select => write!(f, "select"),
            Operator::Pick(depth) => write!(f, "pick {}", depth),
            Operator::Swap(depth) => write!(f, "swap {}", depth),
            Operator::Load { ty, memarg } => {
                write!(f, "{}.load {}, {}", ty, memarg.flags, memarg.offset)
            }
            Operator::Load8 { ty, memarg } => {
                write!(f, "{}.load8 {}, {}", ty, memarg.flags, memarg.offset)
            }
            Operator::Load16 { ty, memarg } => {
                write!(f, "{}.load16 {}, {}", ty, memarg.flags, memarg.offset)
            }
            Operator::Load32 { sign, memarg } => write!(
                f,
                "{}.load32 {}, {}",
                SignfulInt(*sign, Size::_64),
                memarg.flags,
                memarg.offset
            ),
            Operator::Store { ty, memarg } => {
                write!(f, "{}.store {}, {}", ty, memarg.flags, memarg.offset)
            }
            Operator::Store8 { ty, memarg } => write!(
                f,
                "{}.store8 {}, {}",
                SignfulInt(Signedness::Unsigned, *ty),
                memarg.flags,
                memarg.offset
            ),
            Operator::Store16 { ty, memarg } => write!(
                f,
                "{}.store16 {}, {}",
                SignfulInt(Signedness::Unsigned, *ty),
                memarg.flags,
                memarg.offset
            ),
            Operator::Store32 { memarg } => {
                write!(f, "u64.store32 {}, {}", memarg.flags, memarg.offset)
            }
            Operator::MemorySize { .. } => write!(f, "memory.size"),
            Operator::MemoryGrow { .. } => write!(f, "memory.grow"),
            Operator::Const(val) => write!(f, "const {}", val),
            Operator::Eq(ty) => write!(f, "{}.eq", ty),
            Operator::Ne(ty) => write!(f, "{}.ne", ty),
            Operator::Eqz(ty) => write!(f, "{}.eqz", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Lt(ty) => write!(f, "{}.lt", ty),
            Operator::Gt(ty) => write!(f, "{}.gt", ty),
            Operator::Le(ty) => write!(f, "{}.le", ty),
            Operator::Ge(ty) => write!(f, "{}.ge", ty),
            Operator::Add(ty) => write!(f, "{}.add", ty),
            Operator::Sub(ty) => write!(f, "{}.sub", ty),
            Operator::Mul(ty) => write!(f, "{}.mul", ty),
            Operator::Clz(ty) => write!(f, "{}.clz", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Ctz(ty) => write!(f, "{}.ctz", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Popcnt(ty) => write!(f, "{}.popcnt", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Div(ty) => write!(f, "{}.div", ty),
            Operator::Rem(ty) => write!(f, "{}.rem", ty),
            Operator::And(ty) => write!(f, "{}.and", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Or(ty) => write!(f, "{}.or", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Xor(ty) => write!(f, "{}.xor", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Shl(ty) => write!(f, "{}.shl", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Shr(ty) => write!(f, "{}.shr", ty),
            Operator::Rotl(ty) => write!(f, "{}.rotl", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Rotr(ty) => write!(f, "{}.rotr", SignfulInt(Signedness::Unsigned, *ty)),
            Operator::Abs(ty) => write!(f, "{}.abs", Type::<Int>::Float(*ty)),
            Operator::Neg(ty) => write!(f, "{}.neg", Type::<Int>::Float(*ty)),
            Operator::Ceil(ty) => write!(f, "{}.ceil", Type::<Int>::Float(*ty)),
            Operator::Floor(ty) => write!(f, "{}.floor", Type::<Int>::Float(*ty)),
            Operator::Trunc(ty) => write!(f, "{}.trunc", Type::<Int>::Float(*ty)),
            Operator::Nearest(ty) => write!(f, "{}.nearest", Type::<Int>::Float(*ty)),
            Operator::Sqrt(ty) => write!(f, "{}.sqrt", Type::<Int>::Float(*ty)),
            Operator::Min(ty) => write!(f, "{}.min", Type::<Int>::Float(*ty)),
            Operator::Max(ty) => write!(f, "{}.max", Type::<Int>::Float(*ty)),
            Operator::Copysign(ty) => write!(f, "{}.copysign", Type::<Int>::Float(*ty)),
            Operator::I32WrapFromI64 => write!(f, "i32.wrap_from.i64"),
            Operator::F32DemoteFromF64 => write!(f, "f32.demote_from.f64"),
            Operator::F64PromoteFromF32 => write!(f, "f64.promote_from.f32"),
            Operator::I32ReinterpretFromF32 => write!(f, "i32.reinterpret_from.f32"),
            Operator::I64ReinterpretFromF64 => write!(f, "i64.reinterpret_from.f64"),
            Operator::F32ReinterpretFromI32 => write!(f, "f32.reinterpret_from.i32"),
            Operator::F64ReinterpretFromI64 => write!(f, "f64.reinterpret_from.i64"),
            Operator::FConvertFromI {
                input_ty,
                output_ty,
            } => write!(
                f,
                "{}.convert_from.{}",
                Type::Float::<Int>(*output_ty),
                input_ty,
            ),
            Operator::GlobalGet(index) => write!(f, "global.get {}", index),
            Operator::GlobalSet(index) => write!(f, "global.set {}", index),
            Operator::ITruncFromF {
                input_ty,
                output_ty,
            } => write!(
                f,
                "{}.truncate_from.{}",
                output_ty,
                Type::<Int>::Float(*input_ty)
            ),
            Operator::Extend32 { sign } => write!(
                f,
                "{}.extend_from.{}",
                SignfulInt(*sign, Size::_64),
                SignfulInt(*sign, Size::_32)
            ),
            Operator::Extend16 { size } => write!(
                f,
                "{}.extend_from.i16",
                SignfulInt(Signedness::Signed, *size),
            ),
            Operator::Extend8 { size } => write!(
                f,
                "{}.extend_from.i8",
                SignfulInt(Signedness::Signed, *size),
            ),
        }
    }
}

/// Type of a control frame.
#[derive(Debug, Clone, PartialEq)]
enum ControlFrameKind {
    /// A regular block frame.
    ///
    /// Can be used for an implicit function block.
    Block {
        needs_end_label: bool,
    },
    Function,
    /// Loop frame (branching to the beginning of block).
    Loop,
    /// True-subblock of if expression.
    If {
        has_else: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct ControlFrame {
    id: u32,
    arguments: u32,
    returns: Vec<SignlessType>,
    kind: ControlFrameKind,
}

impl ControlFrame {
    fn needs_end_label(&self) -> bool {
        match self.kind {
            ControlFrameKind::Block { needs_end_label } => needs_end_label,
            ControlFrameKind::If { .. } => true,
            ControlFrameKind::Loop | ControlFrameKind::Function => false,
        }
    }

    fn mark_branched_to(&mut self) {
        if let ControlFrameKind::Block { needs_end_label } = &mut self.kind {
            *needs_end_label = true
        }
    }

    fn br_target(&self) -> BrTarget<(u32, NameTag)> {
        match self.kind {
            ControlFrameKind::Loop => BrTarget::Label((self.id, NameTag::Header)),
            ControlFrameKind::Function => BrTarget::Return,
            ControlFrameKind::Block { .. } | ControlFrameKind::If { .. } => {
                BrTarget::Label((self.id, NameTag::End))
            }
        }
    }
}

#[derive(Default)]
struct ControlFrames {
    inner: Vec<ControlFrame>,
}

impl ControlFrames {
    fn function_block(&self) -> &ControlFrame {
        self.inner.first().unwrap()
    }

    fn get(&self, n: usize) -> Option<&ControlFrame> {
        self.inner.iter().rev().nth(n)
    }

    fn get_mut(&mut self, n: usize) -> Option<&mut ControlFrame> {
        self.inner.iter_mut().rev().nth(n)
    }

    fn top(&self) -> Option<&ControlFrame> {
        self.get(0)
    }

    fn top_mut(&mut self) -> Option<&mut ControlFrame> {
        self.get_mut(0)
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn pop(&mut self) -> Option<ControlFrame> {
        self.inner.pop()
    }

    fn push(&mut self, val: ControlFrame) {
        self.inner.push(val)
    }
}

impl std::ops::Index<usize> for ControlFrames {
    type Output = ControlFrame;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl std::ops::IndexMut<usize> for ControlFrames {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index).unwrap()
    }
}

pub struct MicrowasmConv<'a, M> {
    // TODO: Maybe have a `ConvInner` type and have this wrap an `Option` so that
    //       we can dealloc everything when we've finished emitting
    is_done: bool,
    consts_to_emit: Option<Vec<Value>>,
    stack: Vec<SignlessType>,
    operators: OperatorsReader<'a>,
    module: &'a M,
    current_id: u32,
    pointer_type: SignlessType,
    control_frames: ControlFrames,
    unreachable: bool,
}

#[derive(Debug)]
enum SigT {
    T,
    Concrete(SignlessType),
}

impl fmt::Display for SigT {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::T => write!(f, "{{any}}"),
            Self::Concrete(ty) => write!(f, "{}", ty),
        }
    }
}

impl From<SignlessType> for SigT {
    fn from(other: SignlessType) -> SigT {
        SigT::Concrete(other)
    }
}

#[derive(Debug)]
pub struct OpSig {
    input: Vec<SigT>,
    output: Vec<SigT>,
}

impl fmt::Display for OpSig {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(")?;

        let mut iter = self.input.iter();
        if let Some(t) = iter.next() {
            write!(f, "{}", t)?;
        }

        for t in iter {
            write!(f, ", {}", t)?;
        }

        write!(f, ") -> (")?;

        let mut iter = self.output.iter();
        if let Some(t) = iter.next() {
            write!(f, "{}", t)?;
        }

        for t in iter {
            write!(f, ", {}", t)?;
        }

        write!(f, ")")
    }
}

impl OpSig {
    #[inline(always)]
    fn new<I0, I1>(input: I0, output: I1) -> Self
    where
        I0: IntoIterator<Item = SigT>,
        I1: IntoIterator<Item = SigT>,
    {
        OpSig {
            input: Vec::from_iter(input),
            output: Vec::from_iter(output),
        }
    }

    fn none() -> Self {
        Self::new(None, None)
    }
}

impl<T> From<&'_ T> for OpSig
where
    T: Signature,
{
    fn from(other: &T) -> Self {
        OpSig::new(
            other
                .params()
                .iter()
                .map(|t| SigT::Concrete(t.to_microwasm_type())),
            other
                .returns()
                .iter()
                .map(|t| SigT::Concrete(t.to_microwasm_type())),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithLoc<T> {
    pub op: T,
    pub offset: SourceLoc,
}

impl<'a, M: ModuleContext> MicrowasmConv<'a, M>
where
    for<'any> &'any M::Signature: Into<OpSig>,
{
    pub fn new(
        context: &'a M,
        params: impl IntoIterator<Item = SignlessType>,
        returns: impl IntoIterator<Item = SignlessType>,
        func_body: FunctionBody<'a>,
        pointer_type: SignlessType,
    ) -> Result<Self, Error> {
        let mut locals = Vec::from_iter(params);
        let mut consts = Vec::new();

        let local_reader = func_body.get_locals_reader()?;
        let operators = func_body.get_operators_reader()?;

        for loc in local_reader {
            let (count, ty) =
                loc.map_err(|e| Error::Microwasm(format!("Getting local failed: {}", e)))?;
            let ty = Type::from_wasm(ty)
                .map_err(|_| Error::Microwasm("Invalid local type".to_string()))?;

            locals.extend(std::iter::repeat(ty).take(count as _));
            consts.extend(
                std::iter::repeat(ty)
                    .map(Value::default_for_type)
                    .take(count as _),
            )
        }

        let num_locals = locals.len() as _;

        let mut out = Self {
            is_done: false,
            stack: locals,
            module: context,
            consts_to_emit: Some(consts),
            operators,
            current_id: 0,
            control_frames: Default::default(),
            pointer_type,
            unreachable: false,
        };

        let id = out.next_id();
        out.control_frames.push(ControlFrame {
            id,
            arguments: num_locals,
            returns: returns.into_iter().collect(),
            kind: ControlFrameKind::Function,
        });

        Ok(out)
    }

    fn type_or_func_type_to_sig(
        &self,
        ty: wasmparser::TypeOrFuncType,
    ) -> Result<
        (
            impl ExactSizeIterator<Item = SignlessType> + Clone + '_,
            impl ExactSizeIterator<Item = SignlessType> + Clone + '_,
        ),
        Error,
    > {
        match ty {
            wasmparser::TypeOrFuncType::Type(ty) => {
                let mwasm_type = Type::from_wasm_block(ty)?;

                Ok((
                    Either::Left(iter::empty()),
                    Either::Left(mwasm_type.into_iter()),
                ))
            }
            wasmparser::TypeOrFuncType::FuncType(ty) => {
                let sig = self.module.signature(ty);
                Ok((
                    Either::Right(sig.params().iter().map(|t| t.to_microwasm_type())),
                    Either::Right(sig.returns().iter().map(|t| t.to_microwasm_type())),
                ))
            }
        }
    }

    fn op_sig(&self, op: &WasmOperator) -> Result<OpSig, Error> {
        use self::SigT::T;
        use std::iter::{empty as none, once};

        #[inline(always)]
        fn one<A>(a: A) -> impl IntoIterator<Item = SigT>
        where
            A: Into<SigT>,
        {
            once(a.into())
        }

        #[inline(always)]
        fn two<A, B>(a: A, b: B) -> impl IntoIterator<Item = SigT>
        where
            A: Into<SigT>,
            B: Into<SigT>,
        {
            once(a.into()).chain(once(b.into()))
        }

        #[inline(always)]
        fn three<A, B, C>(a: A, b: B, c: C) -> impl IntoIterator<Item = SigT>
        where
            A: Into<SigT>,
            B: Into<SigT>,
            C: Into<SigT>,
        {
            once(a.into()).chain(once(b.into())).chain(once(c.into()))
        }

        macro_rules! sig {
            (@iter $a:expr, $b:expr, $c:expr) => { three($a, $b, $c) };
            (@iter $a:expr, $b:expr) => { two($a, $b) };
            (@iter $a:expr) => { one($a) };
            (@iter) => { none() };
            (($($t:expr),*) -> ($($o:expr),*)) => {
                OpSig::new(sig!(@iter $($t),*), sig!(@iter $($o),*))
            };
        }

        let o = match op {
            WasmOperator::Unreachable => OpSig::none(),
            WasmOperator::Nop => OpSig::none(),

            WasmOperator::Block { ty } | WasmOperator::Loop { ty } => {
                let (input, _) = self.type_or_func_type_to_sig(*ty)?;
                let input = input.map(SigT::Concrete);
                let output = input.clone();

                OpSig::new(input, output)
            }
            WasmOperator::If { ty } => {
                let (input, _) = self.type_or_func_type_to_sig(*ty)?;
                let input = input.map(SigT::Concrete);
                let output = input.clone();

                OpSig::new(input.chain(one(I32)), output)
            }

            WasmOperator::Else | WasmOperator::End => {
                let input = self
                    .control_frames
                    .top()
                    .ok_or_else(|| error("Missing control frame"))?
                    .returns
                    .iter()
                    .copied()
                    .map(SigT::Concrete);
                let output = input.clone();
                OpSig::new(input, output)
            }

            WasmOperator::Br { .. } => OpSig::none(),
            WasmOperator::BrIf { .. } => sig!((I32) -> ()),
            WasmOperator::BrTable { .. } => sig!((I32) -> ()),
            WasmOperator::Return => OpSig::none(),

            WasmOperator::Call { function_index } => {
                let mut func_type = self.module.func_type(*function_index).into();
                func_type.output.reverse();
                func_type
            }
            WasmOperator::CallIndirect { index, .. } => {
                let mut func_type = self.module.signature(*index).into();
                func_type.input.push(I32.into());
                func_type.output.reverse();
                func_type
            }

            WasmOperator::Drop => sig!((T) -> ()),

            // `Select` pops 3 elements and pushes 1
            WasmOperator::Select => sig!((T, T, I32) -> (T)),

            WasmOperator::LocalGet { local_index } => {
                let ty = self.stack[*local_index as usize];

                sig!(() -> (ty))
            }
            WasmOperator::LocalSet { local_index } => {
                let ty = self.stack[*local_index as usize];

                sig!((ty) -> ())
            }
            WasmOperator::LocalTee { local_index } => {
                let ty = self.stack[*local_index as usize];

                sig!((ty) -> (ty))
            }

            WasmOperator::GlobalGet { global_index } => {
                sig!(() -> (self.module.global_type(*global_index).to_microwasm_type()))
            }
            WasmOperator::GlobalSet { global_index } => {
                sig!((self.module.global_type(*global_index).to_microwasm_type()) -> ())
            }

            WasmOperator::F32Load { .. } => sig!((self.pointer_type) -> (F32)),
            WasmOperator::F64Load { .. } => sig!((self.pointer_type) -> (F64)),

            WasmOperator::I32Load { .. }
            | WasmOperator::I32Load8S { .. }
            | WasmOperator::I32Load8U { .. }
            | WasmOperator::I32Load16S { .. }
            | WasmOperator::I32Load16U { .. } => sig!((self.pointer_type) -> (I32)),

            WasmOperator::I64Load { .. }
            | WasmOperator::I64Load8S { .. }
            | WasmOperator::I64Load8U { .. }
            | WasmOperator::I64Load16S { .. }
            | WasmOperator::I64Load16U { .. }
            | WasmOperator::I64Load32S { .. }
            | WasmOperator::I64Load32U { .. } => sig!((self.pointer_type) -> (I64)),

            WasmOperator::F32Store { .. } => sig!((self.pointer_type, F32) -> ()),
            WasmOperator::F64Store { .. } => sig!((self.pointer_type, F64) -> ()),
            WasmOperator::I32Store { .. }
            | WasmOperator::I32Store8 { .. }
            | WasmOperator::I32Store16 { .. } => sig!((self.pointer_type, I32) -> ()),
            WasmOperator::I64Store { .. }
            | WasmOperator::I64Store8 { .. }
            | WasmOperator::I64Store16 { .. }
            | WasmOperator::I64Store32 { .. } => sig!((self.pointer_type, I64) -> ()),

            WasmOperator::MemorySize { .. } => sig!(() -> (self.pointer_type)),
            WasmOperator::MemoryGrow { .. } => sig!((self.pointer_type) -> (self.pointer_type)),

            WasmOperator::I32Const { .. } => sig!(() -> (I32)),
            WasmOperator::I64Const { .. } => sig!(() -> (I64)),
            WasmOperator::F32Const { .. } => sig!(() -> (F32)),
            WasmOperator::F64Const { .. } => sig!(() -> (F64)),

            // WasmOperator::RefNull => {
            //     return Err(BinaryReaderError {
            //         message: "RefNull unimplemented",
            //         offset: None,
            //     })
            // }
            // WasmOperator::RefIsNull => {
            //     return Err(wasm_reader::Error::new (
            //         strerr("RefIsNull unimplemented"),
            //         None,
            //     ))
            // }

            // All comparison operators remove 2 elements and push 1
            WasmOperator::I32Eqz => sig!((I32) -> (I32)),
            WasmOperator::I32Eq
            | WasmOperator::I32Ne
            | WasmOperator::I32LtS
            | WasmOperator::I32LtU
            | WasmOperator::I32GtS
            | WasmOperator::I32GtU
            | WasmOperator::I32LeS
            | WasmOperator::I32LeU
            | WasmOperator::I32GeS
            | WasmOperator::I32GeU => sig!((I32, I32) -> (I32)),

            WasmOperator::I64Eqz => sig!((I64) -> (I32)),
            WasmOperator::I64Eq
            | WasmOperator::I64Ne
            | WasmOperator::I64LtS
            | WasmOperator::I64LtU
            | WasmOperator::I64GtS
            | WasmOperator::I64GtU
            | WasmOperator::I64LeS
            | WasmOperator::I64LeU
            | WasmOperator::I64GeS
            | WasmOperator::I64GeU => sig!((I64, I64) -> (I32)),

            WasmOperator::F32Eq
            | WasmOperator::F32Ne
            | WasmOperator::F32Lt
            | WasmOperator::F32Gt
            | WasmOperator::F32Le
            | WasmOperator::F32Ge => sig!((F32, F32) -> (I32)),

            WasmOperator::F64Eq
            | WasmOperator::F64Ne
            | WasmOperator::F64Lt
            | WasmOperator::F64Gt
            | WasmOperator::F64Le
            | WasmOperator::F64Ge => sig!((F64, F64) -> (I32)),

            WasmOperator::I32Clz | WasmOperator::I32Ctz | WasmOperator::I32Popcnt => {
                sig!((I32) -> (I32))
            }
            WasmOperator::I64Clz | WasmOperator::I64Ctz | WasmOperator::I64Popcnt => {
                sig!((I64) -> (I64))
            }

            WasmOperator::I32Add
            | WasmOperator::I32Sub
            | WasmOperator::I32Mul
            | WasmOperator::I32DivS
            | WasmOperator::I32DivU
            | WasmOperator::I32RemS
            | WasmOperator::I32RemU
            | WasmOperator::I32And
            | WasmOperator::I32Or
            | WasmOperator::I32Xor
            | WasmOperator::I32Shl
            | WasmOperator::I32ShrS
            | WasmOperator::I32ShrU
            | WasmOperator::I32Rotl
            | WasmOperator::I32Rotr => sig!((I32, I32) -> (I32)),

            WasmOperator::I64Add
            | WasmOperator::I64Sub
            | WasmOperator::I64Mul
            | WasmOperator::I64DivS
            | WasmOperator::I64DivU
            | WasmOperator::I64RemS
            | WasmOperator::I64RemU
            | WasmOperator::I64And
            | WasmOperator::I64Or
            | WasmOperator::I64Xor
            | WasmOperator::I64Shl
            | WasmOperator::I64ShrS
            | WasmOperator::I64ShrU
            | WasmOperator::I64Rotl
            | WasmOperator::I64Rotr => sig!((I64, I64) -> (I64)),

            WasmOperator::F32Abs
            | WasmOperator::F32Neg
            | WasmOperator::F32Ceil
            | WasmOperator::F32Floor
            | WasmOperator::F32Trunc
            | WasmOperator::F32Nearest
            | WasmOperator::F32Sqrt => sig!((F32) -> (F32)),

            WasmOperator::F64Abs
            | WasmOperator::F64Neg
            | WasmOperator::F64Ceil
            | WasmOperator::F64Floor
            | WasmOperator::F64Trunc
            | WasmOperator::F64Nearest
            | WasmOperator::F64Sqrt => sig!((F64) -> (F64)),

            WasmOperator::F32Add
            | WasmOperator::F32Sub
            | WasmOperator::F32Mul
            | WasmOperator::F32Div
            | WasmOperator::F32Min
            | WasmOperator::F32Max
            | WasmOperator::F32Copysign => sig!((F32, F32) -> (F32)),

            WasmOperator::F64Add
            | WasmOperator::F64Sub
            | WasmOperator::F64Mul
            | WasmOperator::F64Div
            | WasmOperator::F64Min
            | WasmOperator::F64Max
            | WasmOperator::F64Copysign => sig!((F64, F64) -> (F64)),

            WasmOperator::I32WrapI64 => sig!((I64) -> (I32)),
            WasmOperator::I32TruncF32S | WasmOperator::I32TruncF32U => sig!((F32) -> (I32)),
            WasmOperator::I32TruncF64S | WasmOperator::I32TruncF64U => sig!((F64) -> (I32)),
            WasmOperator::I64ExtendI32S | WasmOperator::I64ExtendI32U => sig!((I32) -> (I64)),
            WasmOperator::I64TruncF32S | WasmOperator::I64TruncF32U => sig!((F32) -> (I64)),
            WasmOperator::I64TruncF64S | WasmOperator::I64TruncF64U => sig!((F64) -> (I64)),
            WasmOperator::F32ConvertI32S | WasmOperator::F32ConvertI32U => sig!((I32) -> (F32)),
            WasmOperator::F32ConvertI64S | WasmOperator::F32ConvertI64U => sig!((I64) -> (F32)),
            WasmOperator::F32DemoteF64 => sig!((F64) -> (F32)),
            WasmOperator::F64ConvertI32S | WasmOperator::F64ConvertI32U => sig!((I32) -> (F64)),
            WasmOperator::F64ConvertI64S | WasmOperator::F64ConvertI64U => sig!((I64) -> (F64)),
            WasmOperator::F64PromoteF32 => sig!((F32) -> (F64)),
            WasmOperator::I32ReinterpretF32 => sig!((F32) -> (I32)),
            WasmOperator::I64ReinterpretF64 => sig!((F64) -> (I64)),
            WasmOperator::F32ReinterpretI32 => sig!((I32) -> (F32)),
            WasmOperator::F64ReinterpretI64 => sig!((I64) -> (F64)),

            WasmOperator::I32Extend8S | WasmOperator::I32Extend16S => sig!((I32) -> (I32)),
            WasmOperator::I64Extend8S | WasmOperator::I64Extend16S | WasmOperator::I64Extend32S => {
                sig!((I64) -> (I64))
            }

            other => {
                return Err(Error::Microwasm(format!(
                    "Opcode unimplemented: {:?}",
                    other
                )))
            }
        };
        Ok(o)
    }

    fn next_id(&mut self) -> u32 {
        let id = self.current_id;
        self.current_id += 1;
        id
    }

    fn local_depth(&self, idx: u32) -> i32 {
        self.stack.len() as i32 - 1 - idx as i32
    }

    fn apply_op(&mut self, op: impl fmt::Debug, sig: OpSig) -> Result<(), Error> {
        let mut ty_param = None;

        for p in sig.input.iter().rev() {
            let stack_ty = match self.stack.pop() {
                Some(e) => e,
                None => return Err(Error::Microwasm("Stack is empty".into())),
            };

            let ty = match p {
                SigT::T => {
                    if let Some(t) = ty_param {
                        t
                    } else {
                        ty_param = Some(stack_ty);
                        stack_ty
                    }
                }
                SigT::Concrete(ty) => *ty,
            };

            if ty != stack_ty {
                return Err(Error::Microwasm(format!(
                    "Error in params for op {:?} (sig {}): expected {}, found {}",
                    op, sig, ty, stack_ty
                )));
            }
        }

        for p in sig.output.into_iter().rev() {
            let ty = match p {
                SigT::T => match ty_param {
                    Some(e) => e,
                    None => return Err(Error::Microwasm("Type parameter was not set".into())),
                },
                SigT::Concrete(ty) => ty,
            };
            self.stack.push(ty);
        }
        Ok(())
    }

    fn block_params(&self) -> Params {
        Params::new(self.stack.iter().cloned())
    }

    fn block_params_with_wasm_type(&self, ty: wasmparser::TypeOrFuncType) -> Result<Params, Error> {
        struct ExactSizeChainIter<A, B> {
            a: A,
            b: B,
        }

        impl<A, B> Iterator for ExactSizeChainIter<A, B>
        where
            A: Iterator,
            B: Iterator<Item = A::Item>,
        {
            type Item = A::Item;

            fn next(&mut self) -> Option<Self::Item> {
                match self.a.next() {
                    Some(val) => Some(val),
                    None => self.b.next(),
                }
            }
        }

        impl<A, B> ExactSizeIterator for ExactSizeChainIter<A, B>
        where
            A: ExactSizeIterator,
            B: ExactSizeIterator<Item = A::Item>,
        {
            fn len(&self) -> usize {
                self.a
                    .len()
                    .checked_add(self.b.len())
                    .expect("Could not chain iterators: sizes overflow `usize`")
            }
        }

        let (params, returns) = self.type_or_func_type_to_sig(ty)?;
        Ok(Params::new(ExactSizeChainIter {
            a: self.stack[0..self.stack.len() - params.len()]
                .iter()
                .copied(),
            b: returns,
        }))
    }

    // Separate from `<Self as Iterator>::next` so we can use `?` to return errors (as
    // `Iterator::next` returns an option and so we'd only be able to use `?` for `None`)
    #[inline(always)]
    fn next(
        &mut self,
    ) -> Result<Option<impl ExactSizeIterator<Item = WithLoc<OperatorFromWasm>> + '_>, Error> {
        use iter_enum::{ExactSizeIterator, Iterator};

        struct Consts {
            inner: <Vec<Value> as IntoIterator>::IntoIter,
        }

        struct WithLocIter<I> {
            iter: I,
            source_loc: SourceLoc,
        }

        impl<I> Iterator for WithLocIter<I>
        where
            I: Iterator,
        {
            type Item = WithLoc<I::Item>;

            fn next(&mut self) -> Option<Self::Item> {
                self.iter.next().map(|op| WithLoc {
                    op,
                    offset: self.source_loc,
                })
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.iter.size_hint()
            }
        }

        impl<I> ExactSizeIterator for WithLocIter<I>
        where
            I: ExactSizeIterator,
        {
            fn len(&self) -> usize {
                self.iter.len()
            }
        }

        impl Iterator for Consts {
            type Item = OperatorFromWasm;

            fn size_hint(&self) -> (usize, Option<usize>) {
                self.inner.size_hint()
            }

            fn next(&mut self) -> Option<Self::Item> {
                self.inner.next().map(Operator::Const)
            }
        }

        impl ExactSizeIterator for Consts {}

        fn consts(consts: Vec<Value>) -> Output {
            Output::Consts(Consts {
                inner: consts.into_iter(),
            })
        }

        fn vec(vals: Vec<OperatorFromWasm>) -> Output {
            Output::Vec(vals.into_iter())
        }

        fn iter(vals: impl IntoIterator<Item = OperatorFromWasm>) -> Output {
            vec(vals.into_iter().collect())
        }

        fn none() -> Output {
            iter(iter::empty())
        }

        fn one(op: OperatorFromWasm) -> Output {
            iter(iter::once(op))
        }

        fn end_if(
            then: BrTargetDrop<WasmLabel>,
            else_: BrTargetDrop<WasmLabel>,
        ) -> OperatorFromWasm {
            Operator::End(Targets {
                targets: [else_].into(),
                default: then,
            })
        }

        #[derive(Iterator, ExactSizeIterator)]
        enum Output {
            Consts(Consts),
            Vec(<Vec<OperatorFromWasm> as IntoIterator>::IntoIter),
        }

        macro_rules! to_drop {
            ($block:expr) => {
                to_drop!($block, self.stack)
            };
            ($block:expr, $stack:expr) => {{
                let block = &$block;
                let len = $stack.len();
                let first_non_local_depth = block.returns.len() as u32;

                (|| {
                    let last_non_local_depth = if block.kind == ControlFrameKind::Function {
                        (len as u32).checked_sub(1)?
                    } else {
                        (len as u32).checked_sub(1)?.checked_sub(block.arguments)?
                    };

                    if first_non_local_depth <= last_non_local_depth {
                        Some(first_non_local_depth..=last_non_local_depth)
                    } else {
                        None
                    }
                })()
            }};
        }

        if let Some(consts_to_emit) = self.consts_to_emit.take() {
            return Ok(Some(WithLocIter {
                iter: consts(consts_to_emit),
                source_loc: Default::default(),
            }));
        }

        if self.unreachable {
            self.unreachable = false;
            let mut depth = 0;

            // `if..then..else`/`br_if` means that there may be branches in which
            // the instruction that caused us to mark this as unreachable to not
            // be executed. Tracking this in the microwasm translation step is
            // very complicated so we just do basic code removal here and leave
            // the removal of uncalled blocks to the backend.
            let (out, offset) = loop {
                if self.is_done {
                    return Ok(None);
                }

                let (op, offset) = self.operators.read_with_offset()?;

                match op {
                    WasmOperator::Block { .. }
                    | WasmOperator::Loop { .. }
                    | WasmOperator::If { .. } => {
                        depth += 1;
                    }
                    WasmOperator::Else => {
                        if depth == 0 {
                            let block = self.control_frames.top_mut().ok_or_else(|| {
                                Error::Microwasm("unreachable Block else Failed".into())
                            })?;

                            self.stack.truncate(block.arguments as _);

                            if let ControlFrameKind::If { has_else, .. } = &mut block.kind {
                                *has_else = true;
                            }

                            break (one(Operator::Start((block.id, NameTag::Else))), offset);
                        }
                    }
                    WasmOperator::End => {
                        if depth == 0 {
                            let block = self.control_frames.pop().ok_or_else(|| {
                                Error::Microwasm("unreachable Block end Failed".into())
                            })?;

                            if self.control_frames.is_empty() {
                                self.is_done = true;
                                return Ok(Some(WithLocIter {
                                    iter: none(),
                                    source_loc: SourceLoc::new(
                                        offset
                                            .try_into()
                                            .expect("Wasm module size overflowed `u32`"),
                                    ),
                                }));
                            }

                            self.stack.truncate(block.arguments as _);
                            self.stack.extend(block.returns);

                            let end_label = (block.id, NameTag::End);

                            if let ControlFrameKind::If {
                                has_else: false, ..
                            } = block.kind
                            {
                                break (
                                    vec(vec![
                                        Operator::Start((block.id, NameTag::Else)),
                                        Operator::Const(0i32.into()),
                                        Operator::End(BrTarget::Label(end_label).into()),
                                        Operator::Start(end_label),
                                    ]),
                                    offset,
                                );
                            } else {
                                break (one(Operator::Start((block.id, NameTag::End))), offset);
                            }
                        } else {
                            depth -= 1;
                        }
                    }
                    _ => {}
                }
            };

            return Ok(Some(WithLocIter {
                iter: out,
                source_loc: SourceLoc::new(
                    offset
                        .try_into()
                        .expect("Wasm module size overflowed `u32`"),
                ),
            }));
        }

        if self.is_done {
            return Ok(None);
        }

        let (op, offset) = self.operators.read_with_offset()?;

        let op_sig = self.op_sig(&op)?;

        self.apply_op(&op, op_sig)
            .map_err(|e| Error::Microwasm(format!("{} (in {:?})", e, op)))?;

        let out = match op {
            WasmOperator::Unreachable => {
                self.unreachable = true;
                one(Operator::Unreachable)
            }
            WasmOperator::Nop => none(),
            WasmOperator::Block { ty } => {
                let id = self.next_id();
                let (_, returns) = self.type_or_func_type_to_sig(ty)?;
                let returns = returns.collect();
                self.control_frames.push(ControlFrame {
                    id,
                    arguments: self.stack.len() as u32,
                    returns,
                    kind: ControlFrameKind::Block {
                        needs_end_label: false,
                    },
                });

                let block_param_type_wasm = self.block_params_with_wasm_type(ty)?;

                one(Operator::end_wasm_block(
                    block_param_type_wasm,
                    (id, NameTag::End),
                ))
            }
            WasmOperator::Loop { ty } => {
                let id = self.next_id();
                let (_, returns) = self.type_or_func_type_to_sig(ty)?;
                let returns = returns.collect();
                self.control_frames.push(ControlFrame {
                    id,
                    arguments: self.stack.len() as u32,
                    returns,
                    kind: ControlFrameKind::Loop,
                });

                let block_param_type_wasm = self.block_params_with_wasm_type(ty)?;
                let label = (id, NameTag::Header);

                vec(vec![
                    Operator::loop_(self.block_params(), label),
                    Operator::end_wasm_block(block_param_type_wasm, (id, NameTag::End)),
                    Operator::Const(0i32.into()),
                    Operator::End(BrTarget::Label(label).into()),
                    Operator::Start(label),
                ])
            }
            WasmOperator::If { ty } => {
                let id = self.next_id();
                let (_, returns) = self.type_or_func_type_to_sig(ty)?;
                let returns = returns.collect();
                self.control_frames.push(ControlFrame {
                    id,
                    arguments: self.stack.len() as u32,
                    returns,
                    kind: ControlFrameKind::If { has_else: false },
                });
                let block_param_type_wasm = self.block_params_with_wasm_type(ty)?;

                let (then, else_, end) = (
                    (id, NameTag::Header),
                    (id, NameTag::Else),
                    (id, NameTag::End),
                );
                vec(vec![
                    Operator::block(self.block_params(), then),
                    Operator::block(self.block_params(), else_),
                    Operator::end_wasm_block(block_param_type_wasm, end),
                    end_if(BrTarget::Label(then).into(), BrTarget::Label(else_).into()),
                    Operator::Start(then),
                ])
            }
            WasmOperator::Else => {
                let block = self
                    .control_frames
                    .top()
                    .ok_or_else(|| Error::Microwasm("Block else Failed".into()))?;
                let to_drop = to_drop!(block);
                let block = self
                    .control_frames
                    .top_mut()
                    .ok_or_else(|| Error::Microwasm("Block else Failed".into()))?;

                if let ControlFrameKind::If { has_else, .. } = &mut block.kind {
                    *has_else = true;
                }

                self.stack.truncate(block.arguments as _);

                let label = (block.id, NameTag::Else);

                iter(
                    to_drop.into_iter().map(Operator::Drop).chain(
                        [
                            Operator::Const(0i32.into()),
                            Operator::End(BrTarget::Label((block.id, NameTag::End)).into()),
                            Operator::Start(label),
                        ]
                        .iter()
                        .cloned(),
                    ),
                )
            }
            WasmOperator::End => {
                let block = self
                    .control_frames
                    .pop()
                    .ok_or_else(|| Error::Microwasm("Block End Failed".into()))?;

                let to_drop = to_drop!(block);

                self.stack.truncate(block.arguments as _);
                self.stack.extend(block.returns.iter().cloned());

                if let ControlFrameKind::If {
                    has_else: false, ..
                } = block.kind
                {
                    let else_ = (block.id, NameTag::Else);
                    let end = (block.id, NameTag::End);

                    iter(
                        to_drop.map(Operator::Drop).into_iter().chain(
                            [
                                Operator::Const(0i32.into()),
                                Operator::End(BrTarget::Label(end).into()),
                                Operator::Start(else_),
                                Operator::Const(0i32.into()),
                                Operator::End(BrTarget::Label(end).into()),
                                Operator::Start(end),
                            ]
                            .iter()
                            .cloned(),
                        ),
                    )
                } else if self.control_frames.is_empty() {
                    self.is_done = true;

                    iter(
                        [
                            Operator::Const(0i32.into()),
                            Operator::End(BrTarget::Return.into()),
                        ]
                        .iter()
                        .cloned(),
                    )
                } else if block.needs_end_label() {
                    let label = (block.id, NameTag::End);

                    iter(
                        to_drop.map(Operator::Drop).into_iter().chain(
                            [
                                Operator::Const(0i32.into()),
                                Operator::End(BrTarget::Label(label).into()),
                                Operator::Start(label),
                            ]
                            .iter()
                            .cloned(),
                        ),
                    )
                } else {
                    iter(to_drop.map(Operator::Drop).into_iter())
                }
            }
            WasmOperator::Br { relative_depth } => {
                self.unreachable = true;
                let to_drop = to_drop!(self.control_frames[relative_depth as _]);

                let block = &mut self.control_frames[relative_depth as _];
                block.mark_branched_to();
                iter(
                    to_drop.into_iter().map(Operator::Drop).chain(
                        [
                            Operator::Const(0i32.into()),
                            Operator::End(block.br_target().into()),
                        ]
                        .iter()
                        .cloned(),
                    ),
                )
            }
            WasmOperator::BrIf { relative_depth } => {
                let to_drop = to_drop!(self.control_frames[relative_depth as _]);

                let label = (self.next_id(), NameTag::Header);
                let params = self.block_params();
                let block = &mut self.control_frames[relative_depth as _];
                block.mark_branched_to();

                vec(vec![
                    Operator::block(params, label),
                    end_if(
                        BrTargetDrop {
                            to_drop,
                            target: block.br_target(),
                        },
                        BrTarget::Label(label).into(),
                    ),
                    Operator::Start(label),
                ])
            }
            WasmOperator::BrTable { table } => {
                self.unreachable = true;
                let (targets, default) = table.read_table()?;
                let control_frames = &mut self.control_frames;
                let stack = &self.stack;
                let targets = targets
                    .iter()
                    .map(|&depth| {
                        control_frames[depth as _].mark_branched_to();
                        let block = &control_frames[depth as _];

                        let target = block.br_target();
                        BrTargetDrop {
                            to_drop: to_drop!(block, stack),
                            target,
                        }
                    })
                    .collect();

                self.control_frames[default as _].mark_branched_to();

                let default = &self.control_frames[default as _];
                let target = default.br_target();
                let default = BrTargetDrop {
                    to_drop: to_drop!(default),
                    target,
                };

                one(Operator::End(Targets { targets, default }))
            }
            WasmOperator::Return => {
                self.unreachable = true;

                let block = self.control_frames.function_block();
                let to_drop = to_drop!(block);

                iter(
                    to_drop.into_iter().map(Operator::Drop).chain(
                        [
                            Operator::Const(0i32.into()),
                            Operator::End(block.br_target().into()),
                        ]
                        .iter()
                        .cloned(),
                    ),
                )
            }
            WasmOperator::Call { function_index } => one(Operator::Call { function_index }),
            WasmOperator::CallIndirect { index, table_index } => one(Operator::CallIndirect {
                type_index: index,
                table_index,
            }),
            WasmOperator::Drop => one(Operator::Drop(0..=0)),
            WasmOperator::Select => one(Operator::Select),

            WasmOperator::LocalGet { local_index } => {
                let depth = self
                    .local_depth(local_index)
                    .checked_sub(1)
                    .ok_or_else(|| Error::Microwasm("LocalGet - Local out of range".into()))?;
                let depth = depth
                    .try_into()
                    .map_err(|_| Error::Microwasm("LocalGet - Local out of range".into()))?;
                one(Operator::Pick(depth))
            }
            WasmOperator::LocalSet { local_index } => {
                let depth = self
                    .local_depth(local_index)
                    .checked_add(1)
                    .ok_or_else(|| Error::Microwasm("LocalSet - Local out of range".into()))?;
                let depth = depth
                    .try_into()
                    .map_err(|_| Error::Microwasm("LocalSet - Local out of range".into()))?;
                vec(vec![Operator::Swap(depth), Operator::Drop(0..=0)])
            }
            WasmOperator::LocalTee { local_index } => {
                let depth = self
                    .local_depth(local_index)
                    .checked_add(1)
                    .ok_or_else(|| Error::Microwasm("LocalTee - Local out of range".into()))?;
                let depth = depth
                    .try_into()
                    .map_err(|_| Error::Microwasm("LocalTee - Local out of range".into()))?;
                vec(vec![
                    Operator::Pick(0),
                    Operator::Swap(depth),
                    Operator::Drop(0..=0),
                ])
            }
            WasmOperator::GlobalGet { global_index } => one(Operator::GlobalGet(global_index)),
            WasmOperator::GlobalSet { global_index } => one(Operator::GlobalSet(global_index)),

            WasmOperator::I32Load { memarg } => one(Operator::Load {
                ty: I32,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load { memarg } => one(Operator::Load {
                ty: I64,
                memarg: memarg.into(),
            }),
            WasmOperator::F32Load { memarg } => one(Operator::Load {
                ty: F32,
                memarg: memarg.into(),
            }),
            WasmOperator::F64Load { memarg } => one(Operator::Load {
                ty: F64,
                memarg: memarg.into(),
            }),
            WasmOperator::I32Load8S { memarg } => one(Operator::Load8 {
                ty: sint::I32,
                memarg: memarg.into(),
            }),
            WasmOperator::I32Load8U { memarg } => one(Operator::Load8 {
                ty: sint::U32,
                memarg: memarg.into(),
            }),
            WasmOperator::I32Load16S { memarg } => one(Operator::Load16 {
                ty: sint::I32,
                memarg: memarg.into(),
            }),
            WasmOperator::I32Load16U { memarg } => one(Operator::Load16 {
                ty: sint::U32,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load8S { memarg } => one(Operator::Load8 {
                ty: sint::I64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load8U { memarg } => one(Operator::Load8 {
                ty: sint::U64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load16S { memarg } => one(Operator::Load16 {
                ty: sint::I64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load16U { memarg } => one(Operator::Load16 {
                ty: sint::U64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load32S { memarg } => one(Operator::Load32 {
                sign: Signedness::Signed,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Load32U { memarg } => one(Operator::Load32 {
                sign: Signedness::Unsigned,
                memarg: memarg.into(),
            }),

            WasmOperator::I32Store { memarg } => one(Operator::Store {
                ty: I32,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Store { memarg } => one(Operator::Store {
                ty: I64,
                memarg: memarg.into(),
            }),
            WasmOperator::F32Store { memarg } => one(Operator::Store {
                ty: F32,
                memarg: memarg.into(),
            }),
            WasmOperator::F64Store { memarg } => one(Operator::Store {
                ty: F64,
                memarg: memarg.into(),
            }),

            WasmOperator::I32Store8 { memarg } => one(Operator::Store8 {
                ty: Size::_32,
                memarg: memarg.into(),
            }),
            WasmOperator::I32Store16 { memarg } => one(Operator::Store16 {
                ty: Size::_32,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Store8 { memarg } => one(Operator::Store8 {
                ty: Size::_64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Store16 { memarg } => one(Operator::Store16 {
                ty: Size::_64,
                memarg: memarg.into(),
            }),
            WasmOperator::I64Store32 { memarg } => one(Operator::Store32 {
                memarg: memarg.into(),
            }),
            WasmOperator::MemorySize { reserved } => one(Operator::MemorySize { reserved }),
            WasmOperator::MemoryGrow { reserved } => one(Operator::MemoryGrow { reserved }),
            WasmOperator::I32Const { value } => one(Operator::Const(Value::I32(value))),
            WasmOperator::I64Const { value } => one(Operator::Const(Value::I64(value))),
            WasmOperator::F32Const { value } => one(Operator::Const(Value::F32(value.into()))),
            WasmOperator::F64Const { value } => one(Operator::Const(Value::F64(value.into()))),
            WasmOperator::RefNull { ty: _ } => {
                return Err(Error::Microwasm("RefNull unimplemented".into()))
            }
            WasmOperator::RefIsNull { ty: _ } => {
                return Err(Error::Microwasm("RefIsNull unimplemented".into()))
            }
            WasmOperator::I32Eqz => one(Operator::Eqz(Size::_32)),
            WasmOperator::I32Eq => one(Operator::Eq(I32)),
            WasmOperator::I32Ne => one(Operator::Ne(I32)),
            WasmOperator::I32LtS => one(Operator::Lt(SI32)),
            WasmOperator::I32LtU => one(Operator::Lt(SU32)),
            WasmOperator::I32GtS => one(Operator::Gt(SI32)),
            WasmOperator::I32GtU => one(Operator::Gt(SU32)),
            WasmOperator::I32LeS => one(Operator::Le(SI32)),
            WasmOperator::I32LeU => one(Operator::Le(SU32)),
            WasmOperator::I32GeS => one(Operator::Ge(SI32)),
            WasmOperator::I32GeU => one(Operator::Ge(SU32)),
            WasmOperator::I64Eqz => one(Operator::Eqz(Size::_64)),
            WasmOperator::I64Eq => one(Operator::Eq(I64)),
            WasmOperator::I64Ne => one(Operator::Ne(I64)),
            WasmOperator::I64LtS => one(Operator::Lt(SI64)),
            WasmOperator::I64LtU => one(Operator::Lt(SU64)),
            WasmOperator::I64GtS => one(Operator::Gt(SI64)),
            WasmOperator::I64GtU => one(Operator::Gt(SU64)),
            WasmOperator::I64LeS => one(Operator::Le(SI64)),
            WasmOperator::I64LeU => one(Operator::Le(SU64)),
            WasmOperator::I64GeS => one(Operator::Ge(SI64)),
            WasmOperator::I64GeU => one(Operator::Ge(SU64)),
            WasmOperator::F32Eq => one(Operator::Eq(F32)),
            WasmOperator::F32Ne => one(Operator::Ne(F32)),
            WasmOperator::F32Lt => one(Operator::Lt(SF32)),
            WasmOperator::F32Gt => one(Operator::Gt(SF32)),
            WasmOperator::F32Le => one(Operator::Le(SF32)),
            WasmOperator::F32Ge => one(Operator::Ge(SF32)),
            WasmOperator::F64Eq => one(Operator::Eq(F64)),
            WasmOperator::F64Ne => one(Operator::Ne(F64)),
            WasmOperator::F64Lt => one(Operator::Lt(SF64)),
            WasmOperator::F64Gt => one(Operator::Gt(SF64)),
            WasmOperator::F64Le => one(Operator::Le(SF64)),
            WasmOperator::F64Ge => one(Operator::Ge(SF64)),
            WasmOperator::I32Clz => one(Operator::Clz(Size::_32)),
            WasmOperator::I32Ctz => one(Operator::Ctz(Size::_32)),
            WasmOperator::I32Popcnt => one(Operator::Popcnt(Size::_32)),
            WasmOperator::I32Add => one(Operator::Add(I32)),
            WasmOperator::I32Sub => one(Operator::Sub(I32)),
            WasmOperator::I32Mul => one(Operator::Mul(I32)),
            WasmOperator::I32DivS => one(Operator::Div(SI32)),
            WasmOperator::I32DivU => one(Operator::Div(SU32)),
            WasmOperator::I32RemS => one(Operator::Rem(sint::I32)),

            WasmOperator::I32RemU => one(Operator::Rem(sint::U32)),
            WasmOperator::I32And => one(Operator::And(Size::_32)),
            WasmOperator::I32Or => one(Operator::Or(Size::_32)),
            WasmOperator::I32Xor => one(Operator::Xor(Size::_32)),
            WasmOperator::I32Shl => one(Operator::Shl(Size::_32)),
            WasmOperator::I32ShrS => one(Operator::Shr(sint::I32)),
            WasmOperator::I32ShrU => one(Operator::Shr(sint::U32)),
            WasmOperator::I32Rotl => one(Operator::Rotl(Size::_32)),
            WasmOperator::I32Rotr => one(Operator::Rotr(Size::_32)),
            WasmOperator::I64Clz => one(Operator::Clz(Size::_64)),
            WasmOperator::I64Ctz => one(Operator::Ctz(Size::_64)),
            WasmOperator::I64Popcnt => one(Operator::Popcnt(Size::_64)),
            WasmOperator::I64Add => one(Operator::Add(I64)),
            WasmOperator::I64Sub => one(Operator::Sub(I64)),
            WasmOperator::I64Mul => one(Operator::Mul(I64)),
            WasmOperator::I64DivS => one(Operator::Div(SI64)),
            WasmOperator::I64DivU => one(Operator::Div(SU64)),
            WasmOperator::I64RemS => one(Operator::Rem(sint::I64)),

            WasmOperator::I64RemU => one(Operator::Rem(sint::U64)),
            WasmOperator::I64And => one(Operator::And(Size::_64)),
            WasmOperator::I64Or => one(Operator::Or(Size::_64)),
            WasmOperator::I64Xor => one(Operator::Xor(Size::_64)),
            WasmOperator::I64Shl => one(Operator::Shl(Size::_64)),
            WasmOperator::I64ShrS => one(Operator::Shr(sint::I64)),
            WasmOperator::I64ShrU => one(Operator::Shr(sint::U64)),
            WasmOperator::I64Rotl => one(Operator::Rotl(Size::_64)),
            WasmOperator::I64Rotr => one(Operator::Rotr(Size::_64)),
            WasmOperator::F32Abs => one(Operator::Abs(Size::_32)),
            WasmOperator::F32Neg => one(Operator::Neg(Size::_32)),
            WasmOperator::F32Ceil => one(Operator::Ceil(Size::_32)),
            WasmOperator::F32Floor => one(Operator::Floor(Size::_32)),
            WasmOperator::F32Trunc => one(Operator::Trunc(Size::_32)),
            WasmOperator::F32Nearest => one(Operator::Nearest(Size::_32)),
            WasmOperator::F32Sqrt => one(Operator::Sqrt(Size::_32)),
            WasmOperator::F32Add => one(Operator::Add(F32)),
            WasmOperator::F32Sub => one(Operator::Sub(F32)),
            WasmOperator::F32Mul => one(Operator::Mul(F32)),
            WasmOperator::F32Div => one(Operator::Div(SF32)),
            WasmOperator::F32Min => one(Operator::Min(Size::_32)),
            WasmOperator::F32Max => one(Operator::Max(Size::_32)),
            WasmOperator::F32Copysign => one(Operator::Copysign(Size::_32)),
            WasmOperator::F64Abs => one(Operator::Abs(Size::_64)),
            WasmOperator::F64Neg => one(Operator::Neg(Size::_64)),
            WasmOperator::F64Ceil => one(Operator::Ceil(Size::_64)),
            WasmOperator::F64Floor => one(Operator::Floor(Size::_64)),
            WasmOperator::F64Trunc => one(Operator::Trunc(Size::_64)),
            WasmOperator::F64Nearest => one(Operator::Nearest(Size::_64)),
            WasmOperator::F64Sqrt => one(Operator::Sqrt(Size::_64)),
            WasmOperator::F64Add => one(Operator::Add(F64)),
            WasmOperator::F64Sub => one(Operator::Sub(F64)),
            WasmOperator::F64Mul => one(Operator::Mul(F64)),
            WasmOperator::F64Div => one(Operator::Div(SF64)),
            WasmOperator::F64Min => one(Operator::Min(Size::_64)),
            WasmOperator::F64Max => one(Operator::Max(Size::_64)),
            WasmOperator::F64Copysign => one(Operator::Copysign(Size::_64)),
            WasmOperator::I32WrapI64 => one(Operator::I32WrapFromI64),
            WasmOperator::I32TruncF32S => one(Operator::ITruncFromF {
                input_ty: Size::_32,
                output_ty: sint::I32,
            }),
            WasmOperator::I32TruncF32U => one(Operator::ITruncFromF {
                input_ty: Size::_32,
                output_ty: sint::U32,
            }),
            WasmOperator::I32TruncF64S => one(Operator::ITruncFromF {
                input_ty: Size::_64,
                output_ty: sint::I32,
            }),
            WasmOperator::I32TruncF64U => one(Operator::ITruncFromF {
                input_ty: Size::_64,
                output_ty: sint::U32,
            }),
            WasmOperator::I64ExtendI32S | WasmOperator::I64Extend32S => one(Operator::Extend32 {
                sign: Signedness::Signed,
            }),
            WasmOperator::I64ExtendI32U => one(Operator::Extend32 {
                sign: Signedness::Unsigned,
            }),
            WasmOperator::I64Extend16S => one(Operator::Extend16 { size: Size::_64 }),
            WasmOperator::I64Extend8S => one(Operator::Extend8 { size: Size::_64 }),
            WasmOperator::I32Extend16S => one(Operator::Extend16 { size: Size::_32 }),
            WasmOperator::I32Extend8S => one(Operator::Extend8 { size: Size::_32 }),
            WasmOperator::I64TruncF32S => one(Operator::ITruncFromF {
                input_ty: Size::_32,
                output_ty: sint::I64,
            }),
            WasmOperator::I64TruncF32U => one(Operator::ITruncFromF {
                input_ty: Size::_32,
                output_ty: sint::U64,
            }),
            WasmOperator::I64TruncF64S => one(Operator::ITruncFromF {
                input_ty: Size::_64,
                output_ty: sint::I64,
            }),
            WasmOperator::I64TruncF64U => one(Operator::ITruncFromF {
                input_ty: Size::_64,
                output_ty: sint::U64,
            }),
            WasmOperator::F32ConvertI32S => one(Operator::FConvertFromI {
                input_ty: sint::I32,
                output_ty: Size::_32,
            }),
            WasmOperator::F32ConvertI32U => one(Operator::FConvertFromI {
                input_ty: sint::U32,
                output_ty: Size::_32,
            }),
            WasmOperator::F32ConvertI64S => one(Operator::FConvertFromI {
                input_ty: sint::I64,
                output_ty: Size::_32,
            }),
            WasmOperator::F32ConvertI64U => one(Operator::FConvertFromI {
                input_ty: sint::U64,
                output_ty: Size::_32,
            }),
            WasmOperator::F64ConvertI32S => one(Operator::FConvertFromI {
                input_ty: sint::I32,
                output_ty: Size::_64,
            }),
            WasmOperator::F64ConvertI32U => one(Operator::FConvertFromI {
                input_ty: sint::U32,
                output_ty: Size::_64,
            }),
            WasmOperator::F64ConvertI64S => one(Operator::FConvertFromI {
                input_ty: sint::I64,
                output_ty: Size::_64,
            }),
            WasmOperator::F64ConvertI64U => one(Operator::FConvertFromI {
                input_ty: sint::U64,
                output_ty: Size::_64,
            }),
            WasmOperator::F32DemoteF64 => one(Operator::F32DemoteFromF64),
            WasmOperator::F64PromoteF32 => one(Operator::F64PromoteFromF32),
            WasmOperator::I32ReinterpretF32 => one(Operator::I32ReinterpretFromF32),
            WasmOperator::I64ReinterpretF64 => one(Operator::I64ReinterpretFromF64),
            WasmOperator::F32ReinterpretI32 => one(Operator::F32ReinterpretFromI32),
            WasmOperator::F64ReinterpretI64 => one(Operator::F64ReinterpretFromI64),

            WasmOperator::I32TruncSatF32S => {
                return Err(Error::Microwasm("I32TruncSatF32S unimplemented".into()))
            }
            WasmOperator::I32TruncSatF32U => {
                return Err(Error::Microwasm("I32TruncSatF32U unimplemented".into()))
            }
            WasmOperator::I32TruncSatF64S => {
                return Err(Error::Microwasm("I32TruncSatF64S unimplemented".into()))
            }
            WasmOperator::I32TruncSatF64U => {
                return Err(Error::Microwasm("I32TruncSatF64U unimplemented".into()))
            }
            WasmOperator::I64TruncSatF32S => {
                return Err(Error::Microwasm("I64TruncSatF32S unimplemented".into()))
            }
            WasmOperator::I64TruncSatF32U => {
                return Err(Error::Microwasm("I64TruncSatF32U unimplemented".into()))
            }
            WasmOperator::I64TruncSatF64S => {
                return Err(Error::Microwasm("I64TruncSatF64S unimplemented".into()))
            }
            WasmOperator::I64TruncSatF64U => {
                return Err(Error::Microwasm("I64TruncSatF64U unimplemented".into()))
            }
            other => {
                return Err(Error::Microwasm(format!(
                    "Opcode unimplemented: {:?}",
                    other
                )))
            }
        };

        Ok(Some(WithLocIter {
            iter: out,
            source_loc: SourceLoc::new(
                offset
                    .try_into()
                    .expect("Wasm module size overflowed `u32`"),
            ),
        }))
    }
}

impl<M: ModuleContext> Iterator for MicrowasmConv<'_, M>
where
    for<'any> &'any M::Signature: Into<OpSig>,
{
    type Item = Result<Vec<WithLoc<OperatorFromWasm>>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next() {
            Ok(Some(ops)) => Some(Ok(ops.collect())),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}
