pub trait EncodePolicy {
    const MASK_OUTGOING: bool;
}

pub trait DecodePolicy {
    const EXPECT_MASKED: bool;
}

#[derive(Copy, Clone, Debug)]
pub struct Client;
impl EncodePolicy for Client {
    const MASK_OUTGOING: bool = true;
}
impl DecodePolicy for Client {
    const EXPECT_MASKED: bool = false;
}

#[derive(Copy, Clone, Debug)]
pub struct Server;
impl EncodePolicy for Server {
    const MASK_OUTGOING: bool = false;
}
impl DecodePolicy for Server {
    const EXPECT_MASKED: bool = true;
}
