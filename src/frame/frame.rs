use super::Opcode;

#[derive(Debug)]
pub(crate) struct Frame {
    pub(crate) opcode: Opcode,
    pub(crate) payload: Vec<u8>,
    pub(crate) is_fin: bool,
}
