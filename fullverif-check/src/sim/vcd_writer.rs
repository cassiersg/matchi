use super::module::{InstanceId, InstanceType, WireId, WireValue};
use super::netlist::Netlist;
use super::recsim::InstanceEvaluatorState;
use super::ModuleId;
use anyhow::Result;
use yosys_netlist_json as yosys;

#[derive(Debug)]
struct VcdBuilder {
    next_id: vcd::IdCode,
}

pub struct VcdWriter<'w> {
    writer: vcd::Writer<&'w mut dyn std::io::Write>,
    top_representation: VcdModuleRepresentation,
    timestamp: u64,
    clock: vcd::IdCode,
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
        instance: String,
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
                    let (subscope, module_representation) =
                        self.module2scope(submodule, instance.name.clone(), netlist, yosys_netlist);
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
        top_scope: String,
        module_id: ModuleId,
        netlist: &Netlist,
        yosys_netlist: &yosys::Netlist,
    ) -> Result<Self> {
        let (top_scope, top_representation) =
            VcdBuilder::new().module2scope(module_id, top_scope, netlist, yosys_netlist);
        let mut writer = vcd::Writer::new(writer);
        writer.scope(&top_scope)?;
        writer.add_module("fv_debug_mod")?;
        let clock = writer.add_wire(1, "clock")?;
        writer.upscope()?;
        writer.timescale(1, vcd::TimescaleUnit::NS)?;
        writer.enddefinitions()?;
        writer.timestamp(0)?;
        Ok(Self {
            writer,
            top_representation,
            timestamp: 0,
            clock,
        })
    }
    fn write_state(
        writer: &mut vcd::Writer<&'w mut dyn std::io::Write>,
        representation: &VcdModuleRepresentation,
        state: &InstanceEvaluatorState,
    ) -> Result<()> {
        for (idcode, wire_ids) in &representation.idcodes {
            let values = wire_ids.iter().rev().map(|wire_id| {
                match state.wire_states[*wire_id].as_ref().unwrap().value {
                    Some(WireValue::_0) => vcd::Value::V0,
                    Some(WireValue::_1) => vcd::Value::V1,
                    None => vcd::Value::X,
                }
            });
            writer.change_vector(*idcode, values)?;
        }
        for (instance_id, sub_representation) in &representation.cells {
            if let Some(instance_state) = state.instance_states[*instance_id].as_ref() {
                Self::write_state(writer, sub_representation, instance_state)?;
            }
        }
        Ok(())
    }
    pub fn new_state(&mut self, state: &InstanceEvaluatorState) -> Result<()> {
        Self::write_state(&mut self.writer, &self.top_representation, state)?;
        self.writer.change_scalar(self.clock, vcd::Value::V1)?;
        self.writer.timestamp(self.timestamp + 5)?;
        self.writer.change_scalar(self.clock, vcd::Value::V0)?;
        self.writer.timestamp(self.timestamp + 10)?;
        self.timestamp += 10;
        Ok(())
    }
}
