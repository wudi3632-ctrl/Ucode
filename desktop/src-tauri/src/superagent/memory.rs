use super::{Decision, Delivery, Effect, Thread, Ticket, Trigger, TriggerDisposition};
use std::collections::{HashMap, HashSet};

/// In-memory repository used by invariant tests and as the behavioral reference
/// for the SQLite adapter.
#[derive(Debug, Default)]
pub struct MemoryRepositories {
    pub triggers: HashMap<String, Trigger>,
    pub trigger_requests: HashMap<String, String>,
    pub dispositions: HashMap<String, TriggerDisposition>,
    pub threads: HashMap<String, Thread>,
    pub decisions: HashMap<String, Decision>,
    pub tickets: HashMap<String, Ticket>,
    pub effects: HashMap<String, Effect>,
    pub effect_keys: HashMap<String, String>,
    pub deliveries: HashMap<String, Delivery>,
    pub delivery_keys: HashSet<String>,
}
