//! RTP and the AEAD-AES-128-GCM SRTP variant of the Nano path.
//!
//! What differs from RFC 7714 and the wire depends on: a 14-byte master salt; the IV built from the
//! session salt's bytes 2–13; one master key and salt for both directions; a sender-owned 48-bit packet
//! index starting at 0 whose low 16 bits are the wire `SEQ`; and the upper layer's 16-bit sequence
//! number carried as a 2-byte little-endian trailer inside the ciphertext. Because both directions
//! share key, salt and starting index and the SSRC carries no direction bit, the two directions can
//! produce identical GCM nonces; the phone decrypts only this construction, so it is kept.

use aes::cipher::{BlockEncrypt, KeyInit as _, generic_array::GenericArray};
use aes_gcm::aead::AeadInOut;
use aes_gcm::{Aes128Gcm, KeyInit};

use super::contract::{MASTER_SALT_LEN, MasterKey};

pub const RTP_HEADER_LEN: usize = 12;
pub const TRAILER_LEN: usize = 2;
pub const TAG_LEN: usize = 16;
/// Bytes SRTP adds around an RTP payload: header, trailer and tag.
pub const OVERHEAD: usize = RTP_HEADER_LEN + TRAILER_LEN + TAG_LEN;
const REPLAY_WINDOW: u64 = 64;
const INDEX_MASK: u64 = (1 << 48) - 1;

/// SRTP key-derivation labels.
mod label {
    pub(super) const ENCRYPTION: u8 = 0x00;
    pub(super) const SALT: u8 = 0x02;
}

/// RFC 3711 §4.3 AES-128-CM PRF with key derivation rate 0: the first `n` bytes of the keystream
/// for `label`.
pub fn derive(master: &MasterKey, label: u8, n: usize) -> Vec<u8> {
    let cipher = aes::Aes128::new(GenericArray::from_slice(&master.key));
    let mut iv = [0u8; 16];
    iv[..MASTER_SALT_LEN].copy_from_slice(&master.salt);
    iv[7] ^= label;

    let mut out = Vec::with_capacity(n + 16);
    let mut counter: u16 = 0;
    while out.len() < n {
        let mut block = iv;
        block[14..].copy_from_slice(&counter.to_be_bytes());
        let mut block = GenericArray::from(block);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
        counter = counter.wrapping_add(1);
    }

    out.truncate(n);
    out
}

/// An RTP packet as the layers above SRTP see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub payload_type: u8,
    pub marker: bool,
    pub timestamp: u32,
    pub ssrc: u32,
    /// The upper layer's sequence number (the encrypted trailer), not the wire `SEQ`.
    pub sequence: u16,
    pub payload: Vec<u8>,
}

/// Why an inbound datagram was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SrtpError {
    #[error("shorter than an SRTP packet")]
    Short,
    #[error("RTP version is not 2")]
    Version,
    #[error("SRTCP packet")]
    Srtcp,
    #[error("CSRC or header extension present")]
    Header,
    #[error("replayed or too old")]
    Replay,
    #[error("authentication failed")]
    Authentication,
}

/// One connection's SRTP state: the session key and salt, the send index, the receive replay window.
pub struct SrtpSession {
    cipher: Aes128Gcm,
    salt: [u8; MASTER_SALT_LEN],
    send_index: u64,
    highest: Option<u64>,
    /// Bit `i` set = index `highest − i` received.
    window: u64,
}

impl SrtpSession {
    pub fn new(master: &MasterKey) -> Self {
        let key = derive(master, label::ENCRYPTION, 16);
        let salt = derive(master, label::SALT, MASTER_SALT_LEN);
        Self {
            cipher: Aes128Gcm::new_from_slice(&key).expect("16-byte key"),
            salt: salt.try_into().expect("14 bytes"),
            send_index: 0,
            highest: None,
            window: 0,
        }
    }

    fn nonce(&self, ssrc: u32, index: u64) -> [u8; 12] {
        let mut x = [0u8; 12];
        x[2..6].copy_from_slice(&ssrc.to_be_bytes());
        x[6..10].copy_from_slice(&((index >> 16) as u32).to_be_bytes());
        x[10..12].copy_from_slice(&(index as u16).to_be_bytes());

        for (b, s) in x.iter_mut().zip(&self.salt[2..]) {
            *b ^= s;
        }
        x
    }

    /// Protects `packet` with the next send index.
    pub fn protect(&mut self, packet: &RtpPacket) -> Vec<u8> {
        let index = self.send_index;
        self.send_index = (self.send_index + 1) & INDEX_MASK;

        let mut out = Vec::with_capacity(OVERHEAD + packet.payload.len());
        out.push(0x80);
        out.push((packet.marker as u8) << 7 | (packet.payload_type & 0x7F));
        out.extend_from_slice(&(index as u16).to_be_bytes());
        out.extend_from_slice(&packet.timestamp.to_be_bytes());
        out.extend_from_slice(&packet.ssrc.to_be_bytes());
        out.extend_from_slice(&packet.payload);
        out.extend_from_slice(&packet.sequence.to_le_bytes());

        let nonce = self.nonce(packet.ssrc, index);
        let (header, body) = out.split_at_mut(RTP_HEADER_LEN);
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce.into(), header, body.into())
            .expect("an RTP payload is far below the GCM limit");
        out.extend_from_slice(&tag);
        out
    }

    /// Checks, decrypts and authenticates one datagram.
    pub fn unprotect(&mut self, datagram: &[u8]) -> Result<RtpPacket, SrtpError> {
        if datagram.len() < OVERHEAD {
            return Err(SrtpError::Short);
        }
        if datagram[0] >> 6 != 2 {
            return Err(SrtpError::Version);
        }
        if (0xC1..=0xDF).contains(&datagram[1]) {
            return Err(SrtpError::Srtcp);
        }
        if datagram[0] & 0x3F != 0 {
            return Err(SrtpError::Header);
        }

        let seq = u16::from_be_bytes([datagram[2], datagram[3]]);
        let ssrc = u32::from_be_bytes(datagram[8..12].try_into().expect("4 bytes"));
        let index = self.estimate_index(seq);
        if !self.fresh(index) {
            return Err(SrtpError::Replay);
        }

        let (header, rest) = datagram.split_at(RTP_HEADER_LEN);
        let (ciphertext, tag) = rest.split_at(rest.len() - TAG_LEN);
        let mut plain = ciphertext.to_vec();
        let nonce = self.nonce(ssrc, index);
        let tag: [u8; TAG_LEN] = tag.try_into().expect("16 bytes");
        self.cipher
            .decrypt_inout_detached(&nonce.into(), header, plain.as_mut_slice().into(), &tag.into())
            .map_err(|_| SrtpError::Authentication)?;

        self.accept(index);
        let trailer = plain.split_off(plain.len() - TRAILER_LEN);
        Ok(RtpPacket {
            payload_type: datagram[1] & 0x7F,
            marker: datagram[1] & 0x80 != 0,
            timestamp: u32::from_be_bytes(datagram[4..8].try_into().expect("4 bytes")),
            ssrc,
            sequence: u16::from_le_bytes([trailer[0], trailer[1]]),
            payload: plain,
        })
    }

    /// RFC 3711 Appendix A: the index from `seq` and the highest index received.
    fn estimate_index(&self, seq: u16) -> u64 {
        let Some(highest) = self.highest else {
            return u64::from(seq);
        };

        let roc = highest >> 16;
        let s_l = highest & 0xFFFF;
        let seq = u64::from(seq);
        let v = if s_l < 32_768 {
            if seq > s_l && seq - s_l > 32_768 { roc.wrapping_sub(1) } else { roc }
        } else if s_l - 32_768 > seq {
            roc + 1
        } else {
            roc
        };

        (v << 16 | seq) & INDEX_MASK
    }

    fn fresh(&self, index: u64) -> bool {
        match self.highest {
            None => true,
            Some(h) if index > h => true,
            Some(h) => {
                let age = h - index;
                age < REPLAY_WINDOW && self.window & (1 << age) == 0
            }
        }
    }

    fn accept(&mut self, index: u64) {
        match self.highest {
            None => {
                self.highest = Some(index);
                self.window = 1;
            }
            Some(h) if index > h => {
                let shift = index - h;
                self.window = if shift >= REPLAY_WINDOW { 0 } else { self.window << shift };
                self.window |= 1;
                self.highest = Some(index);
            }
            Some(h) => self.window |= 1 << (h - index),
        }
    }
}

#[cfg(test)]
mod tests;
