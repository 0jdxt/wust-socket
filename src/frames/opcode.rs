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
