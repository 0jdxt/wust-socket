use std::io::Write;

use bytes::{BufMut, Bytes, BytesMut};
use flate2::write::DeflateEncoder;
use tokio::sync::mpsc::{Sender, error::SendError};

use super::Opcode;
use crate::{MAX_FRAME_PAYLOAD, MAX_MESSAGE_SIZE, role::RolePolicy};

// DataFrames may be fragmented or very large hence they need extra processing compared to ControlFrames
pub(crate) async fn data<R: RolePolicy>(
    data_tx: &Sender<Bytes>,
    payload: &[u8],
    opcode: Opcode,
    deflater: &mut Option<DeflateEncoder<Vec<u8>>>,
    use_context: bool,
) -> Result<(), SendError<Bytes>> {
    if let Some(deflater) = deflater {
        let init_size = payload.len();

        let end = if use_context {
            deflater.get_ref().len()
        } else {
            let _ = deflater.reset(vec![]);
            0
        };

        let _ = deflater.write_all(payload);
        let _ = deflater.flush();
        let _ = deflater.flush();

        let b = &deflater.get_ref()[end..];
        tracing::trace!("deflated {init_size} -> {}", b.len());

        all_frames::<R>(data_tx, opcode, b, true).await
    } else {
        all_frames::<R>(data_tx, opcode, payload, false).await
    }
}

async fn all_frames<R: RolePolicy>(
    data_tx: &Sender<Bytes>,
    opcode: Opcode,
    payload: &[u8],
    compressed: bool,
) -> Result<(), SendError<Bytes>> {
    let mut first = true;
    let mut buf = BytesMut::with_capacity(MAX_MESSAGE_SIZE);

    let (chunked, remainder) = payload.as_chunks::<MAX_FRAME_PAYLOAD>();

    for (i, chunk) in chunked.iter().enumerate() {
        let last = remainder.is_empty() && i == chunked.len() - 1;
        single_frame::<R>(&mut buf, opcode, chunk, &mut first, last, compressed);
        data_tx.send(buf.split().freeze()).await?;
        buf.clear();
    }

    // if there is a remainder, send as last or if empty payload, send empty
    if !remainder.is_empty() || payload.is_empty() {
        single_frame::<R>(&mut buf, opcode, remainder, &mut first, true, compressed);
        data_tx.send(buf.freeze()).await?;
    }
    Ok(())
}

fn single_frame<R: RolePolicy>(
    buf: &mut BytesMut,
    opcode: Opcode,
    chunk: &[u8],
    first: &mut bool,
    last: bool,
    compressed: bool,
) {
    tracing::trace!(
        opcode = ?opcode,
        len = chunk.len(),
        first = first,
        fin = last,
        compressed = compressed,
        "encoding DATA"
    );

    let mut b1 = if *first { opcode } else { Opcode::Cont } as u8;
    b1 |= if last { 0b1000_0000 } else { 0 }; // set FIN
    b1 |= if *first && compressed { 0b0100_0000 } else { 0 }; // set RSV1
    buf.put_u8(b1);
    *first = false; // change AFTER pushing b1

    // push LEN
    #[allow(clippy::cast_possible_truncation)]
    match chunk.len() {
        0..=125 => buf.put_u8(chunk.len() as u8),
        126..=65535 => {
            buf.put_u8(126);
            buf.put_u16(chunk.len() as u16);
        }
        _ => {
            buf.put_u8(127);
            buf.put_u64(chunk.len() as u64);
        }
    }

    // Clients must SEND masked
    if R::CLIENT {
        buf[1] |= 0x80; // set MASK bit
        // get random bytes and push to buf
        let mut mask_key = [0; 4];
        rand::fill(&mut mask_key);
        buf.extend_from_slice(&mask_key);

        // mask bytes
        let start = buf.len();
        buf.extend_from_slice(chunk);
        crate::protocol::mask(&mut buf[start..], mask_key);
    } else {
        buf.extend_from_slice(chunk);
    }
}

#[cfg(test)]
mod bench {
    extern crate test;
    use std::hint::black_box;

    use paste::paste;
    use test::Bencher;
    use tokio::sync::{Mutex, mpsc::channel};

    use super::*;
    use crate::role::*;

    #[allow(clippy::cast_possible_truncation)]
    fn make_payload(len: usize) -> Vec<u8> { (0..len).map(|i| i as u8).collect() }

    fn bench_data_frame<R: RolePolicy>(b: &mut Bencher, payload_len: usize) {
        let payload = make_payload(payload_len);
        let (tx, rx) = channel(1);
        let rx = Mutex::new(rx);
        b.iter(async || {
            data::<R>(&tx, &payload, Opcode::Text, &mut None, false)
                .await
                .unwrap();
            while let Some(bytes) = rx.lock().await.recv().await {
                black_box(bytes);
            }
        });
    }

    macro_rules! bench_data_sizes {
    ($($len:expr),* $(,)?) => {
        $(paste!{
            #[bench]
            fn [<bench_client_ $len>](b: &mut Bencher) {
                bench_data_frame::<Client>(b, $len);
            }
            #[bench]
            fn [<bench_server_ $len>](b: &mut Bencher) {
                bench_data_frame::<Server>(b, $len);
            }
        })*
    };
    }

    bench_data_sizes!(125, 1024, 4096, 16384, 32768);
}
