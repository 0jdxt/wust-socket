#[derive(Copy, Clone, Debug)]
pub(crate) enum Role {
    Client,
    // TODO: server
    #[allow(dead_code)]
    Server,
}

impl Role {
    pub(crate) fn is_client(self) -> bool { matches!(self, Role::Client) }

    pub(crate) fn is_server(self) -> bool { matches!(self, Role::Server) }
}
