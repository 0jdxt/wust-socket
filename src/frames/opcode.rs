#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub(crate) enum Opcode {
    Cont = 0x0,
    Text = 0x1,
    Bin = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    pub(crate) fn is_control(self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }

    #[allow(dead_code)]
    pub(crate) fn is_data(self) -> bool {
        matches!(self, Opcode::Bin | Opcode::Text | Opcode::Cont)
    }
}

impl TryFrom<u8> for Opcode {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x0 => Ok(Opcode::Cont),
            0x1 => Ok(Opcode::Text),
            0x2 => Ok(Opcode::Bin),
            0x8 => Ok(Opcode::Close),
            0x9 => Ok(Opcode::Ping),
            0xA => Ok(Opcode::Pong),
            _ => Err(()),
        }
    }
}
