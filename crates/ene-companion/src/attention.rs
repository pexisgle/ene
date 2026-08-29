//! Attention pipeline for #1199.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] pub enum Priority { Low, Medium, High, Urgent }
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct AttentionItem { pub id: String, pub task_id: String, pub priority: Priority, pub action_required: bool, pub dedupe_key: String, pub payload: String }
#[derive(Debug, Default)] pub struct AttentionStore { items: HashMap<String, AttentionItem> }
impl AttentionStore {
    pub fn push(&mut self, item: AttentionItem) -> bool { if self.items.values().any(|v| v.dedupe_key==item.dedupe_key && v.task_id==item.task_id) { return false; } self.items.insert(item.id.clone(), item); true }
    pub fn pending_for_digest(&self) -> Vec<&AttentionItem> { let mut v: Vec<_>=self.items.values().filter(|i| i.priority==Priority::Low && !i.action_required).collect(); v.sort_by_key(|i| i.id.clone()); v }
    pub fn action_required(&self) -> Vec<&AttentionItem> { self.items.values().filter(|i| i.action_required).collect() }
}
#[derive(Debug, PartialEq, Eq)] pub enum Delivery { SurfaceTurn, Digest, Defer, Persist }
pub fn should_deliver(item: &AttentionItem, user_speaking: bool, quiet: bool, connected: bool) -> Delivery { if !connected { return Delivery::Persist; } if user_speaking && item.priority!=Priority::Urgent { return Delivery::Defer; } if quiet && !item.action_required { return Delivery::Digest; } Delivery::SurfaceTurn }
#[cfg(test)] mod tests {
    use super::*;
    fn item(id: &str, p: Priority, a: bool) -> AttentionItem { AttentionItem{id:id.into(),task_id:"t1".into(),priority:p,action_required:a,dedupe_key:id.into(),payload:"done".into()} }
    #[test] fn action_not_buried(){ let mut s=AttentionStore::default(); s.push(item("low",Priority::Low,false)); s.push(item("urg",Priority::High,true)); assert!(s.action_required().iter().any(|i| i.id=="urg")); assert!(!s.pending_for_digest().iter().any(|i| i.id=="urg")); }
    #[test] fn low_aggregatable(){ let mut s=AttentionStore::default(); s.push(item("a",Priority::Low,false)); s.push(item("b",Priority::Low,false)); assert_eq!(s.pending_for_digest().len(),2); }
    #[test] fn dedupe(){ let mut s=AttentionStore::default(); assert!(s.push(item("x",Priority::Low,false))); let mut dup=item("x",Priority::Low,false); dup.id="x2".into(); dup.dedupe_key="x".into(); assert!(!s.push(dup)); }
    #[test] fn disconnected_persists(){ let it=item("a",Priority::High,true); assert_eq!(should_deliver(&it,false,false,false), Delivery::Persist); }
}
