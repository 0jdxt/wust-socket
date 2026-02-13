pub trait RolePolicy: Send + Sync + 'static + std::fmt::Debug {
    const CLIENT: bool;
    const SERVER: bool;
}

#[derive(Debug)]
pub struct Client;
impl RolePolicy for Client {
    const CLIENT: bool = true;
    const SERVER: bool = false;
}

#[derive(Debug)]
pub struct Server;
impl RolePolicy for Server {
    const CLIENT: bool = false;
    const SERVER: bool = true;
}
