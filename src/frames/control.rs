use std::marker::PhantomData;

use super::Opcode;
use crate::role::RolePolicy;

// -- FAST PATH --
// Separate ControlFrame struct to allow a fast path for sending single frames (Ping, Pong, Close) which
// will have a payload <= 125 bytes and FIN always set.
pub(crate) struct ControlFrame<'a, P: RolePolicy> {
    opcode: Opcode,
    payload: &'a [u8],
    _p: PhantomData<P>,
}

// Functions to produce each kind of ControlFrame with payload,
// and a send function to write to stream
impl<'a, P: RolePolicy> ControlFrame<'a, P> {
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

    // encoding: sets Opcode, FIN, MASK and optionally masks payload
    pub(crate) fn encode(&self) -> Vec<u8> {
        let role = if P::MASK_OUTGOING { "CLI" } else { "SRV" };
        tracing::info!(
            opcode = ?self.opcode,
            len = self.payload.len(),
            "{role} encoding CTRL"
        );

        let mut buf = [0; 131]; // max single frame size
        buf[0] = self.opcode as u8 | 0x80; // always set FIN
        buf[1] = u8::try_from(self.payload.len()).expect("ControlFrame payload too large");

        // Clients must SEND masked
        let header_end = if P::MASK_OUTGOING {
            buf[1] |= 0x80;
            rand::fill(&mut buf[2..6]);
            for (i, b) in self.payload.iter().enumerate() {
                buf[6 + i] = b ^ buf[2 + (i % 4)];
            }
            6
        } else {
            buf[2..2 + self.payload.len()].copy_from_slice(self.payload);
            2
        };
        buf[..header_end + self.payload.len()].to_vec()
    }
}

#[cfg(test)]
mod bench {
    extern crate test;
    use paste::paste;
    use test::{black_box, Bencher};

    use super::*;
    use crate::role::*;

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
