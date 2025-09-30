#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    ReadReady,
    WriteReady,
    Priority,
    HangUp,
    Timeout,
    Process,
}
