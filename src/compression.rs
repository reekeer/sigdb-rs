use crate::error::{Error, Result};
use zstd_safe::{CCtx, CParameter, DCtx};

pub fn compress_zstd(data: &[u8], level: i32) -> Result<Vec<u8>> {
    let fail =
        |code: usize| Error::format(format!("zstd compression failed: {}", zstd_safe::get_error_name(code)));
    let mut cctx = CCtx::create();
    cctx.set_parameter(CParameter::CompressionLevel(level)).map_err(fail)?;
    cctx.set_parameter(CParameter::ContentSizeFlag(true)).map_err(fail)?;
    cctx.set_pledged_src_size(Some(data.len() as u64)).map_err(fail)?;
    let mut out: Vec<u8> = Vec::with_capacity(zstd_safe::compress_bound(data.len()));
    cctx.compress2(&mut out, data).map_err(fail)?;
    Ok(out)
}

pub fn decompress_zstd(data: &[u8], expected_size: usize) -> Result<Vec<u8>> {
    let mismatch = || Error::format("zstd frame size does not match section size");
    let declared =
        zstd_safe::get_frame_content_size(data).map_err(|_| Error::format("invalid zstd frame"))?;
    if let Some(d) = declared {
        if d != expected_size as u64 {
            return Err(mismatch());
        }
    }
    let out = if declared == Some(0) {
        Vec::new()
    } else {
        let failed = |_| Error::format("zstd decompression failed");
        let frame = zstd_safe::find_frame_compressed_size(data).map_err(failed)?;
        let capacity = match declared {
            Some(d) => d as usize,
            None => expected_size.max(1),
        };
        let mut buf: Vec<u8> = Vec::with_capacity(capacity);
        DCtx::create().decompress(&mut buf, &data[..frame.min(data.len())]).map_err(failed)?;
        buf
    };
    if out.len() != expected_size {
        return Err(mismatch());
    }
    Ok(out)
}
