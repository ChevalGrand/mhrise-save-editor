use std::{fmt, io::Cursor, path::Path};

use bitflags::bitflags;
use murmur3::murmur3_32;
use thiserror::Error;

const DSSS_HEADER_LEN: usize = 12;
const FILE_HASH_LEN: usize = 4;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SaveFlags: u32 {
        const BLOWFISH = 0x01;
        const HAS_ID = 0x02;
        const CITRUS = 0x04;
        const DEFLATE = 0x08;
        const MANDARIN = 0x10;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
  NintendoSwitch,
  Steam,
  Auxiliary,
  Unknown,
}

impl fmt::Display for Platform {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let name = match self {
      Self::NintendoSwitch => "Nintendo Switch",
      Self::Steam => "Steam",
      Self::Auxiliary => "Auxiliary",
      Self::Unknown => "Unknown",
    };
    f.write_str(name)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumStatus {
  Valid,
  Invalid { stored: u32, calculated: u32 },
}

impl fmt::Display for ChecksumStatus {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::Valid => f.write_str("valid"),
      Self::Invalid { stored, calculated } => {
        write!(f, "invalid (stored={stored:08x}, calculated={calculated:08x})")
      }
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DsssHeader {
  pub version: u32,
  pub flags: SaveFlags,
  pub raw_flags: u32,
}

impl DsssHeader {
  pub fn platform(self) -> Platform {
    match (self.flags.contains(SaveFlags::DEFLATE), self.flags.contains(SaveFlags::CITRUS)) {
      (true, false) => Platform::NintendoSwitch,
      (false, true) => Platform::Steam,
      (false, false) => Platform::Auxiliary,
      (true, true) => Platform::Unknown,
    }
  }
}

#[derive(Debug, Error)]
pub enum FormatError {
  #[error("file is too small to contain a DSSS header and checksum")]
  TooSmall,
  #[error("invalid DSSS magic")]
  InvalidMagic,
  #[error("unsupported DSSS version {0}")]
  UnsupportedVersion(u32),
  #[error("could not calculate MurmurHash3: {0}")]
  Hash(#[source] std::io::Error),
}

pub fn parse_header(data: &[u8]) -> Result<DsssHeader, FormatError> {
  if data.len() < DSSS_HEADER_LEN + FILE_HASH_LEN {
    return Err(FormatError::TooSmall);
  }
  if &data[..4] != b"DSSS" {
    return Err(FormatError::InvalidMagic);
  }

  let version = u32::from_le_bytes(data[4..8].try_into().expect("fixed-size slice"));
  if version != 2 {
    return Err(FormatError::UnsupportedVersion(version));
  }

  let raw_flags = u32::from_le_bytes(data[8..12].try_into().expect("fixed-size slice"));
  Ok(DsssHeader { version, flags: SaveFlags::from_bits_truncate(raw_flags), raw_flags })
}

pub fn checksum_status(data: &[u8]) -> Result<ChecksumStatus, FormatError> {
  if data.len() < DSSS_HEADER_LEN + FILE_HASH_LEN {
    return Err(FormatError::TooSmall);
  }

  let stored =
    u32::from_le_bytes(data[data.len() - FILE_HASH_LEN..].try_into().expect("fixed-size slice"));
  let calculated = murmur3_32(&mut Cursor::new(&data[..data.len() - FILE_HASH_LEN]), 0xffff_ffff)
    .map_err(FormatError::Hash)?;

  Ok(if stored == calculated {
    ChecksumStatus::Valid
  } else {
    ChecksumStatus::Invalid { stored, calculated }
  })
}

pub fn inspect_path(path: &Path) -> Result<(DsssHeader, ChecksumStatus), FormatError> {
  let data = std::fs::read(path).map_err(FormatError::Hash)?;
  let header = parse_header(&data)?;
  let checksum = checksum_status(&data)?;
  Ok((header, checksum))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn identifies_platform_from_flags() {
    let switch =
      DsssHeader { version: 2, flags: SaveFlags::DEFLATE, raw_flags: SaveFlags::DEFLATE.bits() };
    let steam =
      DsssHeader { version: 2, flags: SaveFlags::CITRUS, raw_flags: SaveFlags::CITRUS.bits() };
    assert_eq!(switch.platform(), Platform::NintendoSwitch);
    assert_eq!(steam.platform(), Platform::Steam);
  }

  #[test]
  fn rejects_non_dsss_data() {
    assert!(matches!(parse_header(b"not-a-save"), Err(FormatError::TooSmall)));
    assert!(matches!(
      parse_header(b"NOPE\x02\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
      Err(FormatError::InvalidMagic)
    ));
  }
}
