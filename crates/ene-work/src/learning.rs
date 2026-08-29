//! Learning candidate for #1205.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct LearningCandidate { pub id: String, pub draft: String, pub evaluated: bool, pub approved: bool, pub version: u32 }
#[derive(Debug, Default)] pub struct CandidateStore { pub items: Vec<LearningCandidate> }
impl CandidateStore { pub fn promote(&mut self, id: &str) -> bool { if let Some(c)=self.items.iter_mut().find(|c| c.id==id) { if !c.evaluated || !c.approved { return false; } c.version+=1; return true; } false } pub fn rollback(&mut self, id: &str) -> bool { if let Some(c)=self.items.iter_mut().find(|c| c.id==id) { if c.version>0 { c.version-=1; return true; } } false } }
#[cfg(test)] mod tests { use super::*; #[test] fn no_auto(){ let mut s=CandidateStore{items:vec![LearningCandidate{id:"c1".into(),draft:"x".into(),evaluated:false,approved:false,version:0}]}; assert!(!s.promote("c1")); } #[test] fn rollback(){ let mut s=CandidateStore{items:vec![LearningCandidate{id:"c1".into(),draft:"x".into(),evaluated:true,approved:true,version:1}]}; assert!(s.promote("c1")); assert!(s.rollback("c1")); assert_eq!(s.items[0].version,1); } }
