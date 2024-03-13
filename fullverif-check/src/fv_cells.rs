use super::module::{ConnectionId, InputSlice, OutputSlice, WireName};
use super::WireValue;
use anyhow::{bail, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombUnitary {
    Buf,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombBinary {
    And,
    //    Nand,
    Or,
    //    Nor,
    Xor,
    //    Xnor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    CombUnitary(CombUnitary),
    CombBinary(CombBinary),
    Mux,
    Dff,
}

impl std::str::FromStr for Gate {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use CombBinary::*;
        use CombUnitary::*;
        Ok(match s {
            "BUF" => Self::CombUnitary(Buf),
            "NOT" => Self::CombUnitary(Not),
            "AND" => Self::CombBinary(And),
            //            "NAND" => Self::CombBinary(Nand),
            "OR" => Self::CombBinary(Or),
            //            "NOR" => Self::CombBinary(Nor),
            "XOR" => Self::CombBinary(Xor),
            //            "XNOR" => Self::CombBinary(Xnor),
            "MUX" => Self::Mux,
            "DFF" => Self::Dff,
            _ => bail!("'{}' is not a fv_cells gate.", s),
        })
    }
}

impl Gate {
    pub fn is_gate(s: impl AsRef<str>) -> bool {
        s.as_ref().parse::<Gate>().is_ok()
    }
    pub fn connections(&self) -> &'static [WireName<&'static str>] {
        const WA: WireName<&'static str> = WireName::single_port("A");
        const WB: WireName<&'static str> = WireName::single_port("B");
        const WS: WireName<&'static str> = WireName::single_port("S");
        const WY: WireName<&'static str> = WireName::single_port("Y");
        const WC: WireName<&'static str> = WireName::single_port("C");
        const WD: WireName<&'static str> = WireName::single_port("D");
        const WQ: WireName<&'static str> = WireName::single_port("Q");
        match self {
            Gate::CombUnitary(_) => [WA, WY].as_slice(),
            Gate::CombBinary(_) => [WA, WB, WY].as_slice(),
            Gate::Mux => [WA, WB, WS, WY].as_slice(),
            Gate::Dff => [WC, WD, WQ].as_slice(),
        }
    }
    pub fn input_ports(&self) -> &'static InputSlice<ConnectionId> {
        const UNITARY_INPUTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(0)];
        const BINARY_INPUTS: [ConnectionId; 2] = [
            ConnectionId::from_raw_unchecked(0),
            ConnectionId::from_raw_unchecked(1),
        ];
        const MUX_INPUTS: [ConnectionId; 3] = [
            ConnectionId::from_raw_unchecked(0),
            ConnectionId::from_raw_unchecked(1),
            ConnectionId::from_raw_unchecked(2),
        ];
        const DFF_INPUTS: [ConnectionId; 2] = [
            ConnectionId::from_raw_unchecked(0),
            ConnectionId::from_raw_unchecked(1),
        ];
        InputSlice::from_slice(match self {
            Gate::CombUnitary(_) => UNITARY_INPUTS.as_slice(),
            Gate::CombBinary(_) => BINARY_INPUTS.as_slice(),
            Gate::Mux => MUX_INPUTS.as_slice(),
            Gate::Dff => DFF_INPUTS.as_slice(),
        })
    }
    pub fn output_ports(&self) -> &'static OutputSlice<ConnectionId> {
        const UNITARY_OUTPUTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(1)];
        const BINARY_OUTPUTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(2)];
        const MUX_OUTPUTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(3)];
        const DFF_OUTPUTS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(2)];
        OutputSlice::from_slice(match self {
            Gate::CombUnitary(_) => UNITARY_OUTPUTS.as_slice(),
            Gate::CombBinary(_) => BINARY_OUTPUTS.as_slice(),
            Gate::Mux => MUX_OUTPUTS.as_slice(),
            Gate::Dff => DFF_OUTPUTS.as_slice(),
        })
    }
    pub fn comb_deps(&self) -> &'static [ConnectionId] {
        const UNITARY_DEPS: [ConnectionId; 1] = [ConnectionId::from_raw_unchecked(0)];
        const BINARY_DEPS: [ConnectionId; 2] = [
            ConnectionId::from_raw_unchecked(0),
            ConnectionId::from_raw_unchecked(1),
        ];
        const MUX_DEPS: [ConnectionId; 3] = [
            ConnectionId::from_raw_unchecked(0),
            ConnectionId::from_raw_unchecked(1),
            ConnectionId::from_raw_unchecked(2),
        ];
        const DFF_DEPS: [ConnectionId; 0] = [];
        match self {
            Gate::CombUnitary(_) => UNITARY_DEPS.as_slice(),
            Gate::CombBinary(_) => BINARY_DEPS.as_slice(),
            Gate::Mux => MUX_DEPS.as_slice(),
            Gate::Dff => DFF_DEPS.as_slice(),
        }
    }
    pub fn clock(&self) -> Option<WireName<&'static str>> {
        match self {
            Gate::CombUnitary(_) | Gate::CombBinary(_) | Gate::Mux => None,
            Gate::Dff => Some(WireName::single_port("C")),
        }
    }
}

impl CombBinary {
    pub fn neutral(&self) -> WireValue {
        match self {
            CombBinary::And => WireValue::_1,
            CombBinary::Or | CombBinary::Xor => WireValue::_0,
        }
    }
    pub fn absorb(&self) -> Option<WireValue> {
        match self {
            CombBinary::And => Some(WireValue::_0),
            CombBinary::Or => Some(WireValue::_1),
            CombBinary::Xor => None,
        }
    }
    pub fn opx(&self, op0: Option<WireValue>, op1: Option<WireValue>) -> Option<WireValue> {
        match (self, op0, op1) {
            (CombBinary::And, Some(op0), Some(op1)) => Some(op0 & op1),
            (CombBinary::And, Some(WireValue::_0), None)
            | (CombBinary::And, None, Some(WireValue::_0)) => Some(WireValue::_0),
            (CombBinary::And, Some(WireValue::_1), None)
            | (CombBinary::And, None, Some(WireValue::_1))
            | (CombBinary::And, None, None) => None,
            (CombBinary::Or, Some(op0), Some(op1)) => Some(op0 | op1),
            (CombBinary::Or, Some(WireValue::_1), None)
            | (CombBinary::Or, None, Some(WireValue::_1)) => Some(WireValue::_1),
            (CombBinary::Or, Some(WireValue::_0), None)
            | (CombBinary::Or, None, Some(WireValue::_0))
            | (CombBinary::Or, None, None) => None,
            (CombBinary::Xor, Some(op0), Some(op1)) => Some(op0 ^ op1),
            (CombBinary::Xor, None, _) | (CombBinary::Xor, _, None) => None,
        }
    }
}
