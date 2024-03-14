use super::module::WireId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireValue {
    _0,
    _1,
}

impl WireValue {
    pub fn from_vcd(value: vcd::Value) -> Option<Self> {
        match value {
            vcd::Value::V0 => Some(Self::_0),
            vcd::Value::V1 => Some(Self::_1),
            vcd::Value::X | vcd::Value::Z => None,
        }
    }
}

impl std::convert::From<WireValue> for WireId {
    fn from(value: WireValue) -> Self {
        match value {
            WireValue::_0 => WireId::from_usize(0),
            WireValue::_1 => WireId::from_usize(1),
        }
    }
}

impl std::convert::From<WireValue> for bool {
    fn from(value: WireValue) -> Self {
        match value {
            WireValue::_0 => false,
            WireValue::_1 => true,
        }
    }
}

impl std::convert::From<bool> for WireValue {
    fn from(value: bool) -> Self {
        if value {
            WireValue::_1
        } else {
            WireValue::_0
        }
    }
}

impl std::ops::Not for WireValue {
    type Output = Self;
    fn not(self) -> Self::Output {
        match self {
            Self::_0 => Self::_1,
            Self::_1 => Self::_0,
        }
    }
}
impl std::ops::BitAnd for WireValue {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self::Output {
        if self == Self::_0 || rhs == Self::_0 {
            Self::_0
        } else {
            Self::_1
        }
    }
}
impl std::ops::BitOr for WireValue {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        if self == Self::_1 || rhs == Self::_1 {
            Self::_1
        } else {
            Self::_0
        }
    }
}
impl std::ops::BitXor for WireValue {
    type Output = Self;
    fn bitxor(self, rhs: Self) -> Self::Output {
        if self == rhs {
            Self::_0
        } else {
            Self::_1
        }
    }
}
