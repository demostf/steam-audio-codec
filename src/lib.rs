pub use crate::error::SteamAudioError;
use opus::{Channels, Decoder};
use std::fmt::Debug;

mod error;

#[derive(Debug)]
#[repr(u8)]
enum PacketType {
    Silence = 0,
    OpusPlc = 6,
    SampleRate = 11,
}

impl TryFrom<u8> for PacketType {
    type Error = SteamAudioError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Silence),
            6 => Ok(Self::OpusPlc),
            11 => Ok(Self::SampleRate),
            _ => Err(SteamAudioError::UnknownPacketType { ty: value }),
        }
    }
}

fn read_bytes<const N: usize>(data: &[u8]) -> Result<([u8; N], &[u8]), SteamAudioError> {
    if data.len() < N {
        Err(SteamAudioError::InsufficientData)
    } else {
        let (result, rest) = data.split_at(N);
        Ok((result.try_into().unwrap(), rest))
    }
}

fn read_u16(data: &[u8]) -> Result<(u16, &[u8]), SteamAudioError> {
    let (bytes, data) = read_bytes(data)?;
    Ok((u16::from_le_bytes(bytes), data))
}

#[derive(Debug)]
pub enum Packet<'a> {
    /// A number of samples of silence
    Silence(u16),
    /// Opus PLC data
    OpusPlc(SteamOpusData<'a>),
    /// The sample rate for the opus packets
    SampleRate(u16),
}

impl<'a> Packet<'a> {
    pub fn read(data: &'a [u8]) -> Result<(Self, &'a [u8]), SteamAudioError> {
        let ty = PacketType::try_from(*data.first().ok_or(SteamAudioError::InsufficientData)?)?;
        let data = &data[1..];

        let (next, data) = read_u16(data)?;

        Ok(match ty {
            PacketType::Silence => (Packet::Silence(next), data),
            PacketType::OpusPlc => {
                if data.len() < next as usize {
                    return Err(SteamAudioError::InsufficientData);
                } else {
                    let (result, data) = data.split_at(next as usize);
                    (Packet::OpusPlc(SteamOpusData { data: result }), data)
                }
            }
            PacketType::SampleRate => (Packet::SampleRate(next), data),
        })
    }
}

#[derive(Debug)]
pub struct SteamVoiceData<'a> {
    pub steam_id: u64,
    packet_data: &'a [u8],
}

impl<'a> SteamVoiceData<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, SteamAudioError> {
        let (data, crc_data) = data.split_at(data.len() - 4);
        let expected_crc = u32::from_le_bytes(crc_data.try_into().unwrap());
        let calculated_crc = crc32b(data);
        if expected_crc != calculated_crc {
            return Err(SteamAudioError::CrcMismatch {
                actual: calculated_crc,
                expected: expected_crc,
            });
        }

        let (steam_id_bytes, data) = read_bytes(data)?;
        let steam_id = u64::from_le_bytes(steam_id_bytes);
        Ok(SteamVoiceData {
            steam_id,
            packet_data: data,
        })
    }

    /// Get the voice
    pub fn packets(&self) -> impl Iterator<Item = Result<Packet<'a>, SteamAudioError>> {
        SteamPacketIterator {
            data: self.packet_data,
        }
    }
}

struct SteamPacketIterator<'a> {
    data: &'a [u8],
}

impl Debug for SteamPacketIterator<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamPacketIterator")
            .field("data_length", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Iterator for SteamPacketIterator<'a> {
    type Item = Result<Packet<'a>, SteamAudioError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            None
        } else {
            match Packet::read(self.data) {
                Ok((packet, rest)) => {
                    self.data = rest;
                    Some(Ok(packet))
                }
                Err(e) => Some(Err(e)),
            }
        }
    }
}

fn crc32b(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (-((crc & 1) as i32)) as u32;
            crc = (crc >> 1) ^ (0xEDB88320 & mask);
        }
    }
    !crc
}

#[derive(Default)]
pub struct SteamVoiceDecoder {
    decoder: Option<Decoder>,
    sample_rate: u16,
    seq: u16,
}

pub struct SteamOpusData<'a> {
    data: &'a [u8],
}

impl Debug for SteamOpusData<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamOpusData")
            .field("data_length", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl SteamVoiceDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(
        &mut self,
        voice_data: SteamVoiceData,
        output_buffer: &mut [i16],
    ) -> Result<usize, SteamAudioError> {
        let mut total = 0;
        for packet in voice_data.packets() {
            let packet = packet?;
            match packet {
                Packet::SampleRate(rate) => {
                    if self.sample_rate != rate {
                        self.decoder = Some(Decoder::new(rate as u32, Channels::Mono)?);
                        self.sample_rate = rate;
                    }
                }
                Packet::OpusPlc(opus) => {
                    let count = self.decode_opus(opus.data, &mut output_buffer[total..])?;
                    total += count;
                    if total >= output_buffer.len() {
                        return Err(SteamAudioError::InsufficientOutputBuffer);
                    }
                }
                Packet::Silence(silence) => {
                    total += silence as usize;
                }
            }
        }
        Ok(total)
    }

    fn decode_opus(
        &mut self,
        mut data: &[u8],
        output_buffer: &mut [i16],
    ) -> Result<usize, SteamAudioError> {
        let mut total = 0;
        let Some(decoder) = self.decoder.as_mut() else {
            return Err(SteamAudioError::NoSampleRate);
        };

        while data.len() > 2 {
            let (len, remainder) = read_u16(data)?;
            data = remainder;
            if len == u16::MAX {
                decoder.reset_state()?;
                self.seq = 0;
                continue;
            }
            let (seq, remainder) = read_u16(data)?;
            data = remainder;

            if seq < self.seq {
                decoder.reset_state()?;
            } else {
                let lost = seq - self.seq;
                for _ in 0..lost {
                    let count = decoder.decode(&[], &mut output_buffer[total..], false)?;
                    total += count;
                    if total >= output_buffer.len() {
                        return Err(SteamAudioError::InsufficientOutputBuffer);
                    }
                }
            }
            let len = len as usize;

            self.seq = seq + 1;

            if data.len() < len {
                return Err(SteamAudioError::InsufficientData);
            }

            let count = decoder.decode(&data[0..len], &mut output_buffer[total..], false)?;
            data = &data[len..];
            total += count;
            if total >= output_buffer.len() {
                return Err(SteamAudioError::InsufficientOutputBuffer);
            }
        }

        Ok(total)
    }
}
