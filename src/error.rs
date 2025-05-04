use thiserror::Error;

#[derive(Debug, Error)]
pub enum SteamAudioError {
    #[error("crc mismatch for packet, got {actual}, expected: {expected}")]
    CrcMismatch { expected: u32, actual: u32 },
    #[error("insufficient number of bytes provided")]
    InsufficientData,
    #[error("insufficient space in output buffer")]
    InsufficientOutputBuffer,
    #[error("unknown packet type {ty}")]
    UnknownPacketType { ty: u8 },
    #[error(transparent)]
    Opus(#[from] opus::Error),
    #[error("audio data received before sample rate is set")]
    NoSampleRate,
}
