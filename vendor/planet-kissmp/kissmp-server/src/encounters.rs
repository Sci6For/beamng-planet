//! Transactional encounter frames. Resource verification and engine readiness are distinct receipts.
use crate::{planet_session::random_id, world_authority::WorldAuthority};
use anyhow::{bail, Result};
use shared::planet::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};
use tokio::sync::Mutex;
pub type Controls = Vec<(Id, Control)>;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GroupAction {
    Join,
    Split,
}
pub fn prefetch_distance(closing: f64, p99: f64) -> f64 {
    (closing.max(0.) * (p99.max(0.) + 5.) + 512.)
        .max(2000.)
        .min(5000.)
}
pub fn physical_ready(distance: f64, verified_collision: bool) -> bool {
    distance.is_finite() && distance <= 250. && distance >= 0. && verified_collision
}
pub fn encounter_action(
    shared: bool,
    distance: f64,
    closing: f64,
    p99: f64,
) -> Option<GroupAction> {
    let d = prefetch_distance(closing, p99);
    if !shared && distance <= d {
        Some(GroupAction::Join)
    } else if shared && distance > d + 1000. {
        Some(GroupAction::Split)
    } else {
        None
    }
}
pub fn frame_at(p: [f64; 3], id: Id, epoch: Epoch) -> FrameDefinition {
    let (s, c) = p[0].to_radians().sin_cos();
    let (so, co) = p[1].to_radians().sin_cos();
    let n = 6378137. / (1. - 6.6943799901413165e-3 * s * s).sqrt();
    FrameDefinition {
        frame_id: id,
        frame_epoch: epoch,
        origin_wgs84: p,
        origin_ecef: [
            (n + p[2]) * c * co,
            (n + p[2]) * c * so,
            (n * (1. - 6.6943799901413165e-3) + p[2]) * s,
        ],
        enu_basis: [[-so, co, 0.], [-s * co, -s * so, c], [c * co, c * so, s]],
        height_quantum: 0.0625,
    }
}
pub fn ecef(p: [f64; 3]) -> [f64; 3] {
    frame_at(p, Id::default(), Epoch(1)).origin_ecef
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt()
}
struct Transaction {
    frame: FrameDefinition,
    members: Vec<Id>,
    resources: HashSet<Id>,
    local: HashSet<Id>,
    applied: HashSet<Id>,
    committed: bool,
    started: f64,
    retries: u8,
}
#[derive(Default)]
pub struct EncounterCoordinator {
    transactions: HashMap<Id, Transaction>,
    current: HashMap<Id, FrameDefinition>,
    pub interests: HashMap<Id, [f64; 3]>,
    blocked: HashSet<Id>,
    last_distance: Option<(f64, f64)>,
    samples: Vec<f64>,
}
impl EncounterCoordinator {
    pub fn begin(
        &mut self,
        op: Id,
        frame: FrameDefinition,
        members: Vec<Id>,
        now: f64,
    ) -> Result<()> {
        validate_frame(&frame).map_err(anyhow::Error::msg)?;
        if members.is_empty()
            || members.len() > 2
            || self.transactions.contains_key(&op)
            || members.iter().any(|id| self.busy(*id))
        {
            bail!("frame busy")
        };
        self.transactions.insert(
            op,
            Transaction {
                frame,
                members,
                resources: HashSet::new(),
                local: HashSet::new(),
                applied: HashSet::new(),
                committed: false,
                started: now,
                retries: 0,
            },
        );
        Ok(())
    }
    fn busy(&self, id: Id) -> bool {
        self.blocked.contains(&id) || self.transactions.values().any(|t| t.members.contains(&id))
    }
    pub fn ready(&mut self, id: Id, op: Id, resource: bool, local: bool) -> Result<Controls> {
        let t = self
            .transactions
            .get_mut(&op)
            .ok_or_else(|| anyhow::anyhow!("stale ready"))?;
        if !t.members.contains(&id) || t.committed {
            bail!("stale ready")
        };
        if resource {
            t.resources.insert(id);
        }
        if local {
            t.local.insert(id);
        }
        if t.members
            .iter()
            .all(|id| t.resources.contains(id) && t.local.contains(id))
        {
            t.committed = true;
            Ok(t.members
                .iter()
                .map(|id| (*id, Control::CommitFrame { operation: op }))
                .collect())
        } else {
            Ok(vec![])
        }
    }
    pub fn applied(&mut self, id: Id, op: Id) -> Result<Controls> {
        let t = self
            .transactions
            .get_mut(&op)
            .ok_or_else(|| anyhow::anyhow!("stale applied"))?;
        if !t.committed || !t.members.contains(&id) || !t.applied.insert(id) {
            bail!("unexpected applied")
        };
        if t.applied.len() != t.members.len() {
            return Ok(vec![]);
        };
        let t = self.transactions.remove(&op).unwrap();
        for id in &t.members {
            self.current.insert(*id, t.frame.clone());
        }
        Ok(t.members
            .into_iter()
            .map(|id| (id, Control::Resume { operation: op }))
            .collect())
    }
    pub fn cancel(&mut self, op: Id, reason: FixedError) -> Controls {
        if let Some(t) = self.transactions.remove(&op) {
            for id in &t.members {
                self.blocked.insert(*id);
            }
            t.members
                .into_iter()
                .map(|id| {
                    (
                        id,
                        Control::Abort {
                            operation: op,
                            reason: reason.clone(),
                        },
                    )
                })
                .collect()
        } else {
            vec![]
        }
    }
    pub fn expire(&mut self, now: f64) -> Controls {
        let ops: Vec<_> = self
            .transactions
            .iter()
            .filter(|(_, t)| now - t.started >= 120.)
            .map(|(id, _)| *id)
            .collect();
        ops.into_iter()
            .flat_map(|op| self.cancel(op, FixedError::Timeout))
            .collect()
    }
    pub fn disconnect(&mut self, id: Id) -> Controls {
        let ops: Vec<_> = self
            .transactions
            .iter()
            .filter(|(_, t)| t.members.contains(&id))
            .map(|(op, _)| *op)
            .collect();
        let mut out = vec![];
        for op in ops {
            out.extend(self.cancel(op, FixedError::Aborted));
        }
        self.current.remove(&id);
        self.interests.remove(&id);
        self.blocked.remove(&id);
        out.retain(|(peer, _)| *peer != id);
        out
    }
    pub fn retry_quantum(
        &mut self,
        op: Id,
        quantum: f64,
        now: f64,
    ) -> Result<(Id, FrameDefinition, Vec<(Id, [f64; 3])>)> {
        let old = self
            .transactions
            .remove(&op)
            .ok_or_else(|| anyhow::anyhow!("stale relief failure"))?;
        if old.committed
            || old.retries >= 4
            || !quantum.is_finite()
            || quantum <= old.frame.height_quantum
            || quantum > 1.
        {
            self.transactions.insert(op, old);
            bail!("relief retry exhausted")
        }
        let mut frame = old.frame.clone();
        frame.frame_id = random_id();
        frame.frame_epoch = Epoch(
            frame
                .frame_epoch
                .0
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("frame epoch overflow"))?,
        );
        frame.height_quantum = quantum;
        let members = old.members.clone();
        let bindings = members
            .iter()
            .map(|id| {
                (
                    *id,
                    self.interests
                        .get(id)
                        .copied()
                        .unwrap_or(old.frame.origin_wgs84),
                )
            })
            .collect();
        let next = random_id();
        self.begin(next, frame.clone(), members, now)?;
        self.transactions.get_mut(&next).unwrap().retries = old.retries + 1;
        Ok((next, frame, bindings))
    }
    pub fn members(&self) -> HashSet<Id> {
        self.current
            .keys()
            .chain(self.interests.keys())
            .copied()
            .chain(
                self.transactions
                    .values()
                    .flat_map(|t| t.members.iter().copied()),
            )
            .collect()
    }
    pub fn sample(&mut self, op: Id, now: f64) {
        if let Some(t) = self.transactions.get(&op) {
            let value = now - t.started;
            if value.is_finite() && value >= 0. {
                self.samples.push(value);
                if self.samples.len() > 64 {
                    self.samples.remove(0);
                }
            }
        }
    }
    pub fn frame(&self, op: Id) -> Option<FrameDefinition> {
        self.transactions.get(&op).map(|t| t.frame.clone())
    }
    fn p99(&self) -> f64 {
        if self.samples.is_empty() {
            15.
        } else {
            let mut v = self.samples.clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[((v.len() - 1) as f64 * 0.99).ceil() as usize]
        }
    }
    pub fn update(&mut self, now: f64) -> Vec<(Id, FrameDefinition, Vec<(Id, [f64; 3])>)> {
        let mut ids: Vec<_> = self
            .current
            .keys()
            .filter(|id| self.interests.contains_key(id) && !self.busy(**id))
            .copied()
            .collect();
        ids.sort_by_key(|id| id.0);
        let mut groups = vec![];
        if ids.len() == 2 {
            let a = ids[0];
            let b = ids[1];
            let d = distance(ecef(self.interests[&a]), ecef(self.interests[&b]));
            let closing = self
                .last_distance
                .map(|(old, t)| {
                    if now > t {
                        ((old - d) / (now - t)).max(0.)
                    } else {
                        0.
                    }
                })
                .unwrap_or(0.);
            self.last_distance = Some((d, now));
            let shared = self.current[&a].frame_id == self.current[&b].frame_id;
            match encounter_action(shared, d, closing, self.p99()) {
                Some(GroupAction::Join) => {
                    groups.push(ids.clone());
                }
                Some(GroupAction::Split) => {
                    groups.push(vec![a]);
                    groups.push(vec![b]);
                }
                None => {}
            }
        }
        if groups.is_empty() {
            for id in &ids {
                if distance(ecef(self.interests[id]), self.current[id].origin_ecef) >= 11000. {
                    let peers: Vec<_> = ids
                        .iter()
                        .filter(|p| self.current[p].frame_id == self.current[id].frame_id)
                        .copied()
                        .collect();
                    if !groups.iter().any(|g| g.contains(id)) {
                        groups.push(peers)
                    }
                }
            }
        }
        let mut out = vec![];
        for members in groups {
            let epoch = Epoch(
                members
                    .iter()
                    .map(|id| self.current[id].frame_epoch.0)
                    .max()
                    .unwrap()
                    + 1,
            );
            let mut centre = self.interests[&members[0]];
            centre[2] = 0.;
            let frame = frame_at(centre, random_id(), epoch);
            let op = random_id();
            let bindings = members.iter().map(|id| (*id, self.interests[id])).collect();
            if self.begin(op, frame.clone(), members, now).is_ok() {
                out.push((op, frame, bindings));
            }
        }
        out
    }
}
async fn emit(world: &WorldAuthority, controls: Controls) -> Result<()> {
    for (id, c) in controls {
        world.server.emit_control(id, c).await?;
    }
    Ok(())
}
pub fn install(world: Arc<WorldAuthority>) {
    let state = Arc::new(Mutex::new(EncounterCoordinator::default()));
    let start = Instant::now();
    let s = state.clone();
    world.set_prepare_hook(Arc::new(move |id, op, frame, _w| {
        let s = s.clone();
        Box::pin(async move {
            s.lock()
                .await
                .begin(op, frame, vec![id], start.elapsed().as_secs_f64())
        })
    }));
    let s = state.clone();
    world.set_resource_hook(Arc::new(move |id, op, _frame, w| {
        let s = s.clone();
        Box::pin(async move {
            let mut c = s.lock().await;
            let controls = c.ready(id, op, true, false)?;
            if !controls.is_empty() {
                c.sample(op, start.elapsed().as_secs_f64());
            }
            drop(c);
            emit(&w, controls).await
        })
    }));
    let s = state.clone();
    world.set_frame_failure_hook(Arc::new(move |_id, op, quantum, w| {
        let s = s.clone();
        Box::pin(async move {
            let mut c = s.lock().await;
            if c.frame(op).is_none() {
                return Ok(());
            }
            let retry = c.retry_quantum(op, quantum, start.elapsed().as_secs_f64());
            drop(c);
            match retry {
                Ok((operation, frame, members)) => {
                    for (id, centre) in members {
                        w.server
                            .emit_control(
                                id,
                                Control::PrepareFrame {
                                    operation,
                                    frame: frame.clone(),
                                    centre_wgs84: centre,
                                },
                            )
                            .await?;
                        w.prepare_frame_for(id, operation, frame.clone(), centre)
                            .await?;
                    }
                    Ok(())
                }
                Err(e) => {
                    let aborts = s.lock().await.cancel(op, FixedError::ResourceRejected);
                    emit(&w, aborts).await?;
                    Err(e)
                }
            }
        })
    }));
    let s = state.clone();
    world.set_control_hook(Arc::new(move |id, message, w| {
        let s = s.clone();
        Box::pin(async move {
            let mut c = s.lock().await;
            let controls = match message.command {
                Control::LocalReady { operation } => {
                    let controls = c.ready(id, operation, false, true)?;
                    if !controls.is_empty() {
                        c.sample(operation, start.elapsed().as_secs_f64());
                    }
                    controls
                }
                Control::Leave {} => c.disconnect(id),
                Control::FrameApplied { operation } => {
                    let f = c
                        .frame(operation)
                        .ok_or_else(|| anyhow::anyhow!("stale applied"))?;
                    let controls = c.applied(id, operation)?;
                    drop(c);
                    w.server.set_frame(id, f.frame_id, f.frame_epoch).await?;
                    return emit(&w, controls).await;
                }
                Control::Abort { operation, .. } => c.cancel(operation, FixedError::Aborted),
                Control::Interest { centre_wgs84, .. } => {
                    c.interests.insert(id, centre_wgs84);
                    vec![]
                }
                _ => bail!("unsupported frame control"),
            };
            drop(c);
            emit(&w, controls).await
        })
    }));
    let weak = Arc::downgrade(&world);
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            timer.tick().await;
            let w = match weak.upgrade() {
                Some(w) => w,
                None => break,
            };
            let clients = w.clients().await;
            let mut c = state.lock().await;
            let gone: Vec<_> = c
                .members()
                .into_iter()
                .filter(|id| !clients.contains(id))
                .collect();
            let mut controls = vec![];
            for id in gone {
                controls.extend(c.disconnect(id));
            }
            controls.extend(c.expire(start.elapsed().as_secs_f64()));
            let prepares = c.update(start.elapsed().as_secs_f64());
            drop(c);
            let _ = emit(&w, controls).await;
            for (op, frame, members) in prepares {
                for (id, centre) in members {
                    let result = async {
                        w.server
                            .emit_control(
                                id,
                                Control::PrepareFrame {
                                    operation: op,
                                    frame: frame.clone(),
                                    centre_wgs84: centre,
                                },
                            )
                            .await?;
                        w.prepare_frame_for(id, op, frame.clone(), centre).await
                    }
                    .await;
                    if result.is_err() {
                        let aborts = state.lock().await.cancel(op, FixedError::ResourceRejected);
                        let _ = emit(&w, aborts).await;
                        break;
                    }
                }
            }
        }
    });
}
