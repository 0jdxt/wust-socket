use std::marker::PhantomData;

use super::Opcode;
use crate::{error::CloseReason, role::RolePolicy};

// Separate ControlFrame struct to allow a fast path for sending single frames (Ping, Pong, Close) which will have a payload <= 125 bytes and FIN always set.
pub(crate) struct ControlFrame<'a, R: RolePolicy> {
    opcode: Opcode,
    payload: &'a [u8],
    _p: PhantomData<R>,
}

// Functions to produce each kind of ControlFrame with payload,
// and a send function to write to stream
impl<'a, R: RolePolicy> ControlFrame<'a, R> {
    pub(crate) fn ping(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Ping,
            payload,
            _p: PhantomData,
        }
    }

    pub(crate) fn pong(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Pong,
            payload,
            _p: PhantomData,
        }
    }

    pub(crate) fn close(payload: &'a [u8]) -> Self {
        Self {
            opcode: Opcode::Close,
            payload,
            _p: PhantomData,
        }
    }

    pub(crate) fn close_reason(reason: CloseReason, text: &'static str) -> Vec<u8> {
        let mut payload = [0; 125];
        // push code bytes
        let code: [u8; 2] = reason.into();
        payload[..2].copy_from_slice(&code);

        // limit text length to 123
        let len = text.floor_char_boundary(123);
        let bytes = &text.as_bytes()[..len];
        payload[2..2 + len].copy_from_slice(bytes);
        ControlFrame::<R>::close(&payload[..2 + len]).encode()
    }

    // encoding: sets Opcode, FIN, MASK and optionally masks payload
    pub(crate) fn encode(self) -> Vec<u8> {
        tracing::trace!(
            opcode = ?self.opcode,
            len = self.payload.len(),
            "encoding CTRL"
        );

        let mut buf = [0; 131]; // max single frame size
        buf[0] = self.opcode as u8 | 0x80; // always set FIN
        buf[1] = u8::try_from(self.payload.len()).expect("ControlFrame payload too large");

        // Clients must SEND masked
        if R::MASK_OUTGOING {
            buf[1] |= 0x80;
            rand::fill(&mut buf[2..6]);
            for (i, b) in self.payload.iter().enumerate() {
                buf[6 + i] = b ^ buf[2 + (i % 4)];
            }
            buf[..6 + self.payload.len()].to_vec()
        } else {
            buf[2..2 + self.payload.len()].copy_from_slice(self.payload);
            buf[..2 + self.payload.len()].to_vec()
        }
    }
}

#[cfg(test)]
mod bench {
    extern crate test;
    use paste::paste;
    use test::{black_box, Bencher};

    use super::*;
    use crate::role::*;

    #[allow(clippy::cast_possible_truncation)]
    fn make_payload(len: usize) -> Vec<u8> { (0..len).map(|i| i as u8).collect() }

    fn bench_control_frame<P: RolePolicy>(b: &mut Bencher, payload_len: usize) {
        let payload = make_payload(payload_len);
        b.iter(|| {
            let frame = ControlFrame::<P>::ping(&payload);
            let chunks = black_box(frame.encode());
            black_box(chunks.len()); // consume so compiler can't optimize away
        });
    }

    macro_rules! bench_data_sizes {
    ($($len:expr),* $(,)?) => {
        $(paste!{
            #[bench] fn [<bench_client_ $len>](b: &mut Bencher) {
                bench_control_frame::<Client>(b, $len);
            }
            #[bench] fn [<bench_server_ $len>](b: &mut Bencher) {
                bench_control_frame::<Server>(b, $len);
            }
        })*
    };
}

    bench_data_sizes!(0, 1, 4, 16, 32, 64, 125);
}
