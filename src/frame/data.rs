use std::{
    io::{Result, Write},
    net::TcpStream,
};

use super::Opcode;
use crate::role::Role;

// -- SLOW PATH --
// Data Frames may be fragmented or very large hence they need extra processing compared to
// ControlFrames
pub(crate) struct DataFrame<'a> {
    opcode: Opcode,
    payload: &'a [u8],
    role: Role,
}

impl<'a> DataFrame<'a> {
    pub(crate) fn new(payload: &'a [u8], opcode: Opcode, role: Role) -> Self {
        Self {
            opcode,
            payload,
            role,
        }
    }

    pub(crate) fn send(self, stream: &mut TcpStream) -> Result<()> {
        let mut payload = self.payload;
        let mut first = true;
        while !payload.is_empty() {
            // TODO: handle extended lengths
            let chunk_len = payload.len().min(125);
            let chunk = &payload[..chunk_len];

            let opcode = if first { self.opcode } else { Opcode::Cont };

            let mut buf = vec![0; 6 + chunk_len];
            buf[0] = if chunk_len == payload.len() {
                0x80 | opcode as u8
            } else {
                opcode as u8
            };

            buf[1] = u8::try_from(chunk_len).expect("length is less than u8::MAX");
            // Clients must SEND masked
            if self.role.is_client() {
                buf[1] |= 0x80;
                rand::fill(&mut buf[2..6]);
                for (i, &b) in chunk.iter().enumerate() {
                    buf[6 + i] = b ^ buf[2 + (i % 4)];
                }
            }

            stream.write_all(&buf)?;

            payload = &payload[chunk_len..];
            first = false;
        }

        Ok(())
    }
}
