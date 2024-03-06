use super::module::{InstanceId, InstanceType, WireId, WireValue};
use super::netlist::Netlist;
use super::recsim::ModuleState;
use super::simulation::WireState;
use super::ModuleId;
use crate::utils::ShareId;
use anyhow::Result;
use yosys_netlist_json as yosys;

#[derive(Debug)]
struct VcdBuilder {
    next_id: vcd::IdCode,
}

pub struct VcdWriter<'w> {
    writer: vcd::Writer<&'w mut dyn std::io::Write>,
    representations: Vec<(RepresentationTarget, VcdModuleRepresentation)>,
    timestamp: u64,
    clock: vcd::IdCode,
    cycle_count: vcd::IdCode,
}

#[derive(Debug, Clone)]
struct VcdModuleRepresentation {
    idcodes: Vec<(vcd::IdCode, Vec<WireId>)>,
    cells: Vec<(InstanceId, VcdModuleRepresentation)>,
}

impl VcdBuilder {
    fn new() -> Self {
        Self {
            next_id: vcd::IdCode::FIRST,
        }
    }
    fn next_id(&mut self) -> vcd::IdCode {
        let res = self.next_id;
        self.next_id = res.next();
        res
    }
    fn module2scope(
        &mut self,
        module_id: ModuleId,
        instance: &str,
        netlist: &Netlist,
        yosys_netlist: &yosys::Netlist,
    ) -> (vcd::Scope, VcdModuleRepresentation) {
        let yosys_module = &yosys_netlist.modules[&netlist[module_id].name];
        let mut scope = vcd::Scope::new(vcd::ScopeType::Module, format!("\\{}", instance));
        let mut idcodes = vec![];
        for (name, net) in yosys_module.netnames.iter() {
            let ref_index = (net.offset != 0).then_some(if net.bits.len() == 1 {
                vcd::ReferenceIndex::BitSelect(net.offset as i32)
            } else {
                vcd::ReferenceIndex::Range(
                    (net.offset + net.bits.len() - 1) as i32,
                    net.offset as i32,
                )
            });
            let idcode = self.next_id();
            scope.items.push(vcd::ScopeItem::Var(vcd::Var::new(
                vcd::VarType::Wire,
                net.bits.len() as u32,
                idcode,
                format!("\\{}", name),
                ref_index,
            )));
            let wire_ids: Vec<WireId> = net
                .bits
                .iter()
                .map(|bitval| (*bitval).try_into().unwrap())
                .collect();
            idcodes.push((idcode, wire_ids));
        }
        let cells = netlist[module_id]
            .instances
            .iter_enumerated()
            .filter_map(|(instance_id, instance)| {
                if let InstanceType::Module(submodule) = instance.architecture {
                    let (subscope, module_representation) = self.module2scope(
                        submodule,
                        instance.name.as_str(),
                        netlist,
                        yosys_netlist,
                    );
                    scope.items.push(vcd::ScopeItem::Scope(subscope));
                    Some((instance_id, module_representation))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        (scope, VcdModuleRepresentation { idcodes, cells })
    }
}

impl<'w> VcdWriter<'w> {
    pub fn new(
        writer: &'w mut dyn std::io::Write,
        module_id: ModuleId,
        netlist: &Netlist,
        yosys_netlist: &yosys::Netlist,
    ) -> Result<Self> {
        let mut builder = VcdBuilder::new();
        let nshares = netlist.gadget(module_id).unwrap().nshares;
        let mut writer = vcd::Writer::new(writer);
        let representation_targets = [
            RepresentationTarget::Value,
            RepresentationTarget::Random,
            RepresentationTarget::Deterministic,
        ]
        .into_iter()
        .chain((0..nshares).map(|share_id| RepresentationTarget::Share {
            share_id: ShareId::from_raw(share_id),
        }));
        let representations = representation_targets
            .map(|representation_target| {
                let (scope, representation) = builder.module2scope(
                    module_id,
                    format!("{}", representation_target).as_str(),
                    netlist,
                    yosys_netlist,
                );
                writer.scope(&scope)?;
                Ok((representation_target, representation))
            })
            .collect::<Result<Vec<_>>>()?;
        writer.add_module("fv_debug_mod")?;
        let clock = writer.add_wire(1, "clock")?;
        let cycle_count = writer.add_wire(32, "cycle_count")?;
        writer.upscope()?;
        writer.timescale(1, vcd::TimescaleUnit::NS)?;
        writer.enddefinitions()?;
        writer.timestamp(0)?;
        Ok(Self {
            writer,
            representations,
            timestamp: 0,
            clock,
            cycle_count,
        })
    }
    fn write_state(
        writer: &mut vcd::Writer<&'w mut dyn std::io::Write>,
        representation: &VcdModuleRepresentation,
        state: &ModuleState,
        target: RepresentationTarget,
    ) -> Result<()> {
        for (idcode, wire_ids) in &representation.idcodes {
            let values = wire_ids
                .iter()
                .rev()
                .map(|wire_id| target.state2value(state.wire_states[*wire_id].as_ref().unwrap()));
            writer.change_vector(*idcode, values)?;
        }
        for (instance_id, sub_representation) in &representation.cells {
            if let Some(instance_state) = state.instance_states[*instance_id].as_ref() {
                let instance_state = instance_state.module_any().unwrap();
                Self::write_state(writer, sub_representation, instance_state, target)?;
            }
        }
        Ok(())
    }
    pub fn new_state(&mut self, state: &super::top_sim::SimulationState) -> Result<()> {
        for (target, representation) in &self.representations {
            Self::write_state(&mut self.writer, representation, state.module(), *target)?;
        }
        self.writer
            .change_vector(self.cycle_count, int2bits((self.timestamp / 10) as u32))?;
        self.writer.change_scalar(self.clock, vcd::Value::V1)?;
        self.writer.timestamp(self.timestamp + 5)?;
        self.writer.change_scalar(self.clock, vcd::Value::V0)?;
        self.writer.timestamp(self.timestamp + 10)?;
        self.timestamp += 10;
        Ok(())
    }
}

fn int2bits(x: u32) -> impl Iterator<Item = vcd::Value> {
    let x = x.reverse_bits();
    (0..32).map(move |i| {
        if (x >> i) & 0x1 == 0 {
            vcd::Value::V0
        } else {
            vcd::Value::V1
        }
    })
}

#[derive(Debug, Clone, Copy)]
enum RepresentationTarget {
    Value,
    Random,
    Deterministic,
    Share { share_id: ShareId },
}
impl RepresentationTarget {
    fn state2value(&self, state: &WireState) -> vcd::Value {
        match self {
            Self::Value => match state.value {
                Some(WireValue::_0) => vcd::Value::V0,
                Some(WireValue::_1) => vcd::Value::V1,
                None => vcd::Value::X,
            },
            Self::Random => match state.random {
                Some(_) => vcd::Value::V1,
                None => vcd::Value::X,
            },
            Self::Deterministic => {
                if state.deterministic {
                    vcd::Value::V1
                } else {
                    vcd::Value::X
                }
            }
            Self::Share { share_id } => {
                if state.sensitivity.contains(*share_id) {
                    vcd::Value::V1
                } else {
                    vcd::Value::V0
                }
            }
        }
    }
}

impl std::fmt::Display for RepresentationTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepresentationTarget::Value => write!(f, "value"),
            RepresentationTarget::Random => write!(f, "random"),
            RepresentationTarget::Deterministic => write!(f, "deterministic"),
            RepresentationTarget::Share { share_id } => write!(f, "share_{}", share_id),
        }
    }
}
