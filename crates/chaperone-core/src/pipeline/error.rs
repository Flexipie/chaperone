#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error("pipeline schema {0} is not supported; this binary reads schemas 1 and 2")]
    Schema(i64),
    #[error("[{0}] needs schema 2")]
    NeedsSchema2(&'static str),
    #[error("gate {gate} reads {fact}, a fact the projection never sets")]
    UnknownFact { gate: String, fact: String },
    #[error("runner {runner}: {message}")]
    BadRunner { runner: String, message: String },
    #[error("check {check} names unknown runner {runner}")]
    UnknownRunner { check: String, runner: String },
    #[error("check {check} names prompt {prompt}, which this binary does not carry")]
    UnknownPrompt { check: String, prompt: String },
    #[error("gate {gate}: unknown fn {func}")]
    UnknownFn { gate: String, func: String },
    #[error("gate {gate}: {message}")]
    BadArgs { gate: String, message: String },
    #[error("gate {gate}: unknown limit {limit}")]
    UnknownLimit { gate: String, limit: String },
    #[error("gate {gate}: unknown event {event}")]
    UnknownEvent { gate: String, event: String },
    #[error("{from} names unknown gate {gate}")]
    UnknownGate { from: String, gate: String },
    #[error("duplicate gate id {0}")]
    DuplicateGate(String),
    #[error("duplicate transition {id} to {to} on route {route}")]
    DuplicateTransition {
        id: String,
        route: String,
        to: String,
    },
    #[error("required transition {0} is missing")]
    MissingTransition(&'static str),
    #[error("gate {0} is part of a cycle")]
    Cycle(String),
}
