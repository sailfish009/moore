// Copyright (c) 2016-2020 Fabian Schuiki

//! A port mapping generated by an instantiation.

use crate::{
    crate_prelude::*,
    hir::{HirNode, NamedParam, PosParam},
    ParamEnv,
};
use itertools::Itertools;
use std::sync::Arc;

/// A port mapping.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct PortMapping(Vec<(NodeId, NodeEnvId)>);

impl PortMapping {
    /// Find the signal assigned to a port.
    pub fn find(&self, node_id: NodeId) -> Option<NodeEnvId> {
        self.0
            .iter()
            .find(|&&(id, _)| id == node_id)
            .map(|&(_, id)| id)
    }
}

/// A location that implies a port mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortMappingSource<'hir> {
    ModuleInst {
        module: NodeId,
        inst: NodeId,
        env: ParamEnv,
        pos: &'hir [PosParam],
        named: &'hir [NamedParam],
    },
}

pub(crate) fn compute<'gcx>(
    cx: &impl Context<'gcx>,
    src: PortMappingSource<'gcx>,
) -> Result<Arc<PortMapping>> {
    match src {
        PortMappingSource::ModuleInst {
            module,
            inst: _,
            env,
            pos,
            named,
        } => {
            let module = match cx.hir_of(module)? {
                HirNode::Module(m) => m,
                _ => panic!("expected module"),
            };

            // Associate the positional assignments with external ports.
            let pos_iter = pos.iter().enumerate().map(|(index, &(span, assign_id))| {
                match module.ports_new.ext_pos.get(index) {
                    Some(port) => Ok((port.id, (assign_id, env))),
                    None => {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "{} only has {} ports(s)",
                                module.desc_full(),
                                module.ports_new.ext_pos.len()
                            ))
                            .span(span),
                        );
                        Err(())
                    }
                }
            });

            // Associate the named assignments with external ports.
            let named_iter = named.iter().map(|&(_span, name, assign_id)| {
                let names = match module.ports_new.ext_named.as_ref() {
                    Some(x) => x,
                    None => {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "{} requires positional connections",
                                module.desc_full(),
                            ))
                            .span(name.span)
                            .add_note(
                                "The module has unnamed ports which require connecting by position.",
                            )
                            .add_note(format!("Remove `.{}(...)`", name)),
                        );
                        return Err(());
                    }
                };
                match names.get(&name.value) {
                    Some(&index) => Ok((module.ports_new.ext_pos[index].id, (assign_id, env))),
                    None => {
                        cx.emit(
                            DiagBuilder2::error(format!(
                                "no port `{}` in {}",
                                name,
                                module.desc_full(),
                            ))
                            .span(name.span)
                            .add_note(format!(
                                "Declared ports are {}",
                                module
                                    .ports_new
                                    .ext_pos
                                    .iter()
                                    .flat_map(|n| n.name)
                                    .map(|n| format!("`{}`", n))
                                    .format(", ")
                            )),
                        );
                        Err(())
                    }
                }
            });

            // Build a vector of ports.
            let ports: Result<Vec<_>> = pos_iter
                .chain(named_iter)
                .filter_map(|err| match err {
                    Ok((port_id, (Some(assign_id), env))) => {
                        Some(Ok((port_id, assign_id.env(env))))
                    }
                    Ok(_) => None,
                    Err(()) => Some(Err(())),
                })
                .collect();

            Ok(Arc::new(PortMapping(ports?)))
        }
    }
}
