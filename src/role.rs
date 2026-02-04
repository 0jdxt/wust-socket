pub trait RolePolicy: Send + Sync + 'static {
    const MASK_OUTGOING: bool;
    const EXPECT_MASKED: bool;
}

#[derive(Copy, Clone, Debug)]
pub struct Client;
impl RolePolicy for Client {
    const EXPECT_MASKED: bool = false;
    const MASK_OUTGOING: bool = true;
}

#[derive(Copy, Clone, Debug)]
pub struct Server;
impl RolePolicy for Server {
    const EXPECT_MASKED: bool = true;
    const MASK_OUTGOING: bool = false;
}
