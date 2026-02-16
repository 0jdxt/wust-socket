use super::Opcode;
use crate::{error::CloseReason, role::RolePolicy};

// Separate control frames to allow a fast path for sending single frames (Ping, Pong, Close) which will have a payload <= 125 bytes and FIN always set.

// Functions to produce each kind of control frame with given payload
pub(crate) fn ping<R: RolePolicy>(payload: &[u8]) -> Vec<u8> { encode::<R>(Opcode::Ping, payload) }

pub(crate) fn pong<R: RolePolicy>(payload: &[u8]) -> Vec<u8> { encode::<R>(Opcode::Pong, payload) }

pub(crate) fn close<R: RolePolicy>(reason: CloseReason, text: &'static str) -> Vec<u8> {
    let mut payload = [0; 125];

    // push code bytes
    let code: [u8; 2] = reason.into();
    payload[..2].copy_from_slice(&code);

    // limit text length to 123
    let len = text.floor_char_boundary(123);
    let bytes = &text.as_bytes()[..len];
    payload[2..2 + len].copy_from_slice(bytes);
    encode::<R>(Opcode::Close, &payload[..2 + len])
}

// encoding: sets Opcode, FIN, MASK and optionally masks payload
#[allow(clippy::cast_possible_truncation)]
fn encode<R: RolePolicy>(opcode: Opcode, payload: &[u8]) -> Vec<u8> {
    tracing::trace!(
        opcode = ?opcode,
        len = payload.len(),
        "encoding CTRL"
    );

    let mut buf = [0; 131]; // max single frame size
    buf[0] = opcode as u8 | 0x80; // always set FIN
    buf[1] = payload.len() as u8;

    // Clients must SEND masked
    if R::CLIENT {
        buf[1] |= 0x80;
        let mask_key: [u8; 4] = rand::random();
        buf[2..6].copy_from_slice(&mask_key);
        let end = 6 + payload.len();
        let p = &mut buf[6..end];
        p.copy_from_slice(payload);
        crate::protocol::mask(p, mask_key);
        buf[..end].to_vec()
    } else {
        let end = 2 + payload.len();
        buf[2..end].copy_from_slice(payload);
        buf[..end].to_vec()
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
            let frame = black_box(ping::<P>(&payload));
            black_box(frame.len()); // consume so compiler can't optimize away
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
