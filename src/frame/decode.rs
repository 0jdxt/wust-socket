use super::{Frame, Opcode};
use crate::{error::CloseReason, role::Role};

pub(crate) enum FrameParseResult {
    Complete(Frame),
    Incomplete,
    ProtocolError(CloseReason),
}

impl Frame {
    pub(crate) fn parse_bytes(buf: &[u8], role: Role) -> impl Iterator<Item = FrameParseResult> {
        let mut i = 0;
        std::iter::from_fn(move || {
            if i >= buf.len() {
                return None;
            }

            print!("{} ", buf.len());

            // parse header: FIN, opcode, masked, payload len
            let fin = buf[i] & 0x80 != 0;
            let opcode = match buf[i] & 0x0F {
                0x0 => Opcode::Cont,
                0x1 => Opcode::Text,
                0x2 => Opcode::Bin,
                0x8 => Opcode::Close,
                0x9 => Opcode::Ping,
                0xA => Opcode::Pong,
                _ => {
                    return Some(FrameParseResult::ProtocolError(CloseReason::ProtoError));
                }
            };
            i += 1;

            // Servers must NOT mask message
            let masked = (buf[i] & 0x80) != 0;
            if role.is_server() != masked {
                return Some(FrameParseResult::ProtocolError(CloseReason::ProtoError));
            }

            // parse length (1 byte, 2 byte extended, or 8 byte extended)
            let payload_len = {
                let mut len = (buf[i] & 0x7F) as usize;
                i += 1;

                if len == 126 {
                    let Some(bytes) = buf.get(i..i + 2) else {
                        return Some(FrameParseResult::Incomplete);
                    };
                    len = u16::from_be_bytes(bytes.try_into().unwrap()) as usize;
                    i += 2;
                } else if len == 127 {
                    let Some(bytes) = buf.get(i..i + 8) else {
                        return Some(FrameParseResult::Incomplete);
                    };
                    len = u64::from_be_bytes(bytes.try_into().unwrap()) as usize;
                    i += 8;
                }

                len
            };
            print!("{payload_len} ");

            // validate control frames
            if matches!(opcode, Opcode::Ping | Opcode::Pong | Opcode::Close)
                && (!fin || payload_len > 125)
            {
                return Some(FrameParseResult::ProtocolError(CloseReason::ProtoError));
            }

            let payload = if masked {
                // handle mask
                let Some(mask) = buf.get(i..i + 4) else {
                    return Some(FrameParseResult::Incomplete);
                };
                i += 4;

                match buf.get(i..i + payload_len) {
                    Some(payload) => payload.to_vec(),
                    None => return Some(FrameParseResult::Incomplete),
                }
                .iter()
                .enumerate()
                .map(|(j, b)| b ^ mask[j % 4])
                .collect()
            } else {
                match buf.get(i..i + payload_len) {
                    Some(payload) => payload.to_vec(),
                    None => return Some(FrameParseResult::Incomplete),
                }
            };
            i += payload_len;

            Some(FrameParseResult::Complete(Frame {
                opcode,
                payload,
                is_fin: fin,
            }))
        })
    }
}
