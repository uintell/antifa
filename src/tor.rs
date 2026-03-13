#[derive(Clone, Debug)]
pub enum TorEvent {
    Status(String),
    OnionAddress(String),
}
