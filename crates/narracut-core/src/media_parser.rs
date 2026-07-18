use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STREAM_BUFFER_BYTES: usize = 8 * 1024;
const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;
const MAX_AUDIO_DURATION_MS: u64 = 86_400_000;
const MIN_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_SAMPLE_RATE_HZ: u32 = 384_000;
const MAX_CHANNELS: u16 = 8;
const MAX_CAPTION_CUES: usize = 10_000;
const MAX_CUE_TEXT_CHARS: usize = 2_000;
const MAX_CUE_TEXT_UTF8_BYTES: usize = MAX_CUE_TEXT_CHARS * 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaParseErrorCode {
    Unsupported,
    Io,
    ResourceLimitExceeded,
    InvalidWav,
    InvalidUtf8,
    InvalidSrt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaParseError {
    pub code: MediaParseErrorCode,
    pub message: String,
}

impl MediaParseError {
    fn io() -> Self {
        Self {
            code: MediaParseErrorCode::Io,
            message: "无法读取媒体源".to_owned(),
        }
    }

    fn limit(message: &str) -> Self {
        Self {
            code: MediaParseErrorCode::ResourceLimitExceeded,
            message: message.to_owned(),
        }
    }

    fn invalid_wav(message: &str) -> Self {
        Self {
            code: MediaParseErrorCode::InvalidWav,
            message: message.to_owned(),
        }
    }

    fn invalid_utf8() -> Self {
        Self {
            code: MediaParseErrorCode::InvalidUtf8,
            message: "SRT 必须使用有效的 UTF-8 编码".to_owned(),
        }
    }

    fn invalid_srt(message: &str) -> Self {
        Self {
            code: MediaParseErrorCode::InvalidSrt,
            message: message.to_owned(),
        }
    }
}

impl fmt::Display for MediaParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MediaParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcmWavParseLimits {
    pub max_bytes: u64,
}

impl Default for PcmWavParseLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_MEDIA_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPcmWav {
    pub content_hash: String,
    pub byte_length: u64,
    pub duration_ms: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub byte_rate: u32,
    pub data_byte_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrtParseLimits {
    pub max_bytes: u64,
    pub max_cue_count: usize,
    pub max_cue_text_bytes: usize,
}

impl Default for SrtParseLimits {
    fn default() -> Self {
        Self {
            max_bytes: 4 * 1024 * 1024,
            max_cue_count: 10_000,
            max_cue_text_bytes: MAX_CUE_TEXT_UTF8_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCaptionCue {
    pub cue_id: String,
    pub source_index: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedSrt {
    pub content_hash: String,
    pub byte_length: u64,
    pub cues: Vec<ParsedCaptionCue>,
}

pub fn parse_pcm_wav_file(
    source_path: &Path,
    limits: PcmWavParseLimits,
) -> Result<ParsedPcmWav, MediaParseError> {
    validate_wav_limits(limits)?;

    let file = File::open(source_path).map_err(|_| MediaParseError::io())?;
    let metadata = file.metadata().map_err(|_| MediaParseError::io())?;
    if !metadata.is_file() {
        return Err(MediaParseError::io());
    }
    if metadata.len() == 0 {
        return Err(MediaParseError::invalid_wav("WAV 文件为空"));
    }
    if metadata.len() > limits.max_bytes {
        return Err(MediaParseError::limit("WAV 文件超过读取上限"));
    }

    let mut reader = HashedReader::new(file, limits.max_bytes);
    let mut riff_header = [0_u8; 12];
    reader.read_exact_wav(&mut riff_header)?;
    if &riff_header[0..4] != b"RIFF" {
        return Err(MediaParseError::invalid_wav("缺少 RIFF 文件头"));
    }
    if &riff_header[8..12] != b"WAVE" {
        return Err(MediaParseError::invalid_wav("RIFF 容器不是 WAVE"));
    }

    let riff_payload_bytes = u32::from_le_bytes(
        riff_header[4..8]
            .try_into()
            .expect("RIFF size slice has a fixed length"),
    ) as u64;
    let declared_file_bytes = riff_payload_bytes
        .checked_add(8)
        .ok_or_else(|| MediaParseError::invalid_wav("RIFF 长度溢出"))?;
    if declared_file_bytes < 12 {
        return Err(MediaParseError::invalid_wav("RIFF 长度小于 WAVE 文件头"));
    }
    if declared_file_bytes > limits.max_bytes {
        return Err(MediaParseError::limit("WAV 声明长度超过读取上限"));
    }

    let mut format = None;
    let mut data_byte_length = None;
    while reader.byte_length() < declared_file_bytes {
        let remaining = declared_file_bytes - reader.byte_length();
        if remaining < 8 {
            return Err(MediaParseError::invalid_wav("RIFF 尾部不是完整 chunk"));
        }

        let mut chunk_header = [0_u8; 8];
        reader.read_exact_wav(&mut chunk_header)?;
        let chunk_id: [u8; 4] = chunk_header[0..4]
            .try_into()
            .expect("chunk id slice has a fixed length");
        let chunk_bytes = u32::from_le_bytes(
            chunk_header[4..8]
                .try_into()
                .expect("chunk size slice has a fixed length"),
        ) as u64;
        let padded_chunk_bytes = chunk_bytes
            .checked_add(chunk_bytes & 1)
            .ok_or_else(|| MediaParseError::invalid_wav("chunk 长度溢出"))?;
        let chunk_end = reader
            .byte_length()
            .checked_add(padded_chunk_bytes)
            .ok_or_else(|| MediaParseError::invalid_wav("chunk 边界溢出"))?;
        if chunk_end > declared_file_bytes {
            return Err(MediaParseError::invalid_wav("chunk 超出 RIFF 声明边界"));
        }

        match &chunk_id {
            b"fmt " => {
                if format.is_some() {
                    return Err(MediaParseError::invalid_wav("WAV 包含重复 fmt chunk"));
                }
                if data_byte_length.is_some() {
                    return Err(MediaParseError::invalid_wav(
                        "fmt chunk 位于 data chunk 之后",
                    ));
                }
                if chunk_bytes < 16 {
                    return Err(MediaParseError::invalid_wav("fmt chunk 长度不足"));
                }
                let mut prefix = [0_u8; 16];
                reader.consume_chunk(chunk_bytes, Some(&mut prefix))?;
                format = Some(PcmFormat::parse(prefix)?);
            }
            b"data" => {
                if format.is_none() {
                    return Err(MediaParseError::invalid_wav(
                        "data chunk 位于 fmt chunk 之前",
                    ));
                }
                if data_byte_length.is_some() {
                    return Err(MediaParseError::invalid_wav("WAV 包含重复 data chunk"));
                }
                reader.consume_chunk(chunk_bytes, None)?;
                data_byte_length = Some(chunk_bytes);
            }
            _ => reader.consume_chunk(chunk_bytes, None)?,
        }

        if chunk_bytes & 1 == 1 {
            let mut padding = [0_u8; 1];
            reader.read_exact_wav(&mut padding)?;
        }
    }

    if reader.byte_length() != declared_file_bytes {
        return Err(MediaParseError::invalid_wav("RIFF 长度与 chunk 边界不一致"));
    }
    if reader.read_optional_byte()?.is_some() {
        return Err(MediaParseError::invalid_wav("RIFF 末尾包含未声明数据"));
    }

    let format = format.ok_or_else(|| MediaParseError::invalid_wav("WAV 缺少 fmt chunk"))?;
    let data_byte_length =
        data_byte_length.ok_or_else(|| MediaParseError::invalid_wav("WAV 缺少 data chunk"))?;
    format.validate(data_byte_length)?;

    let frame_count = data_byte_length / u64::from(format.block_align);
    let duration_ms = frame_count
        .checked_mul(1_000)
        .ok_or_else(|| MediaParseError::invalid_wav("WAV 时长计算溢出"))?
        / u64::from(format.sample_rate);
    if duration_ms == 0 || duration_ms > MAX_AUDIO_DURATION_MS {
        return Err(MediaParseError::invalid_wav("WAV 时长超出支持范围"));
    }

    let (byte_length, content_hash) = reader.finish();
    Ok(ParsedPcmWav {
        content_hash,
        byte_length,
        duration_ms,
        channels: format.channels,
        sample_rate: format.sample_rate,
        bits_per_sample: format.bits_per_sample,
        block_align: format.block_align,
        byte_rate: format.byte_rate,
        data_byte_length,
    })
}

fn validate_wav_limits(limits: PcmWavParseLimits) -> Result<(), MediaParseError> {
    if limits.max_bytes == 0 || limits.max_bytes > MAX_MEDIA_BYTES {
        return Err(MediaParseError::limit("WAV 读取上限必须位于支持范围内"));
    }
    Ok(())
}

struct HashedReader<R> {
    inner: R,
    hasher: Sha256,
    byte_length: u64,
    max_bytes: u64,
}

impl<R: Read> HashedReader<R> {
    fn new(inner: R, max_bytes: u64) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            byte_length: 0,
            max_bytes,
        }
    }

    fn byte_length(&self) -> u64 {
        self.byte_length
    }

    fn read_some(&mut self, buffer: &mut [u8]) -> Result<usize, MediaParseError> {
        let read_bytes = loop {
            match self.inner.read(buffer) {
                Ok(read_bytes) => break read_bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(MediaParseError::io()),
            }
        };
        let next_byte_length = self
            .byte_length
            .checked_add(read_bytes as u64)
            .ok_or_else(|| MediaParseError::limit("媒体读取长度溢出"))?;
        if next_byte_length > self.max_bytes {
            return Err(MediaParseError::limit("媒体源超过读取上限"));
        }
        self.hasher.update(&buffer[..read_bytes]);
        self.byte_length = next_byte_length;
        Ok(read_bytes)
    }

    fn read_exact_wav(&mut self, mut buffer: &mut [u8]) -> Result<(), MediaParseError> {
        while !buffer.is_empty() {
            let read_bytes = self.read_some(buffer)?;
            if read_bytes == 0 {
                return Err(MediaParseError::invalid_wav("WAV 数据被截断"));
            }
            buffer = &mut buffer[read_bytes..];
        }
        Ok(())
    }

    fn consume_chunk(
        &mut self,
        chunk_bytes: u64,
        prefix: Option<&mut [u8]>,
    ) -> Result<(), MediaParseError> {
        let mut remaining = chunk_bytes;
        if let Some(prefix) = prefix {
            self.read_exact_wav(prefix)?;
            remaining -= prefix.len() as u64;
        }
        let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
        while remaining > 0 {
            let requested = usize::try_from(remaining.min(STREAM_BUFFER_BYTES as u64))
                .expect("bounded stream request fits usize");
            self.read_exact_wav(&mut buffer[..requested])?;
            remaining -= requested as u64;
        }
        Ok(())
    }

    fn read_optional_byte(&mut self) -> Result<Option<u8>, MediaParseError> {
        let mut byte = [0_u8; 1];
        match self.read_some(&mut byte)? {
            0 => Ok(None),
            _ => Ok(Some(byte[0])),
        }
    }

    fn finish(self) -> (u64, String) {
        let digest = self.hasher.finalize();
        (self.byte_length, format_sha256(&digest))
    }
}

fn format_sha256(digest: &[u8]) -> String {
    let mut hash = String::with_capacity("sha256:".len() + digest.len() * 2);
    hash.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(&mut hash, "{byte:02x}").expect("writing to String cannot fail");
    }
    hash
}

#[derive(Debug, Clone, Copy)]
struct PcmFormat {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    block_align: u16,
    byte_rate: u32,
}

impl PcmFormat {
    fn parse(bytes: [u8; 16]) -> Result<Self, MediaParseError> {
        let audio_format = u16::from_le_bytes([bytes[0], bytes[1]]);
        if audio_format != 1 {
            return Err(MediaParseError::invalid_wav("仅支持未压缩 PCM WAV"));
        }
        Ok(Self {
            channels: u16::from_le_bytes([bytes[2], bytes[3]]),
            sample_rate: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            byte_rate: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            block_align: u16::from_le_bytes([bytes[12], bytes[13]]),
            bits_per_sample: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    fn validate(self, data_byte_length: u64) -> Result<(), MediaParseError> {
        if !(1..=MAX_CHANNELS).contains(&self.channels) {
            return Err(MediaParseError::invalid_wav("PCM 声道数超出支持范围"));
        }
        if !(MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&self.sample_rate) {
            return Err(MediaParseError::invalid_wav("PCM 采样率超出支持范围"));
        }
        if !matches!(self.bits_per_sample, 8 | 16 | 24 | 32) {
            return Err(MediaParseError::invalid_wav("PCM 位深仅支持 8/16/24/32"));
        }

        let expected_block_align = u32::from(self.channels)
            .checked_mul(u32::from(self.bits_per_sample) / 8)
            .ok_or_else(|| MediaParseError::invalid_wav("PCM blockAlign 计算溢出"))?;
        if u32::from(self.block_align) != expected_block_align || self.block_align == 0 {
            return Err(MediaParseError::invalid_wav("PCM blockAlign 不一致"));
        }
        let expected_byte_rate = self
            .sample_rate
            .checked_mul(expected_block_align)
            .ok_or_else(|| MediaParseError::invalid_wav("PCM byteRate 计算溢出"))?;
        if self.byte_rate != expected_byte_rate {
            return Err(MediaParseError::invalid_wav("PCM byteRate 不一致"));
        }
        if data_byte_length == 0 || !data_byte_length.is_multiple_of(u64::from(self.block_align)) {
            return Err(MediaParseError::invalid_wav("PCM data 长度未按帧对齐"));
        }
        Ok(())
    }
}

pub fn parse_srt_file(
    source_path: &Path,
    audio_duration_ms: u64,
    limits: SrtParseLimits,
) -> Result<ParsedSrt, MediaParseError> {
    validate_srt_limits(audio_duration_ms, limits)?;

    let file = File::open(source_path).map_err(|_| MediaParseError::io())?;
    let metadata = file.metadata().map_err(|_| MediaParseError::io())?;
    if !metadata.is_file() {
        return Err(MediaParseError::io());
    }
    if metadata.len() == 0 {
        return Err(MediaParseError::invalid_srt("SRT 文件为空"));
    }
    if metadata.len() > limits.max_bytes {
        return Err(MediaParseError::limit("SRT 文件超过读取上限"));
    }

    let initial_capacity =
        usize::try_from(metadata.len()).expect("media schema byte limit always fits usize");
    let mut bytes = Vec::with_capacity(initial_capacity);
    let mut reader = HashedReader::new(file, limits.max_bytes);
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    loop {
        let read_bytes = reader.read_some(&mut buffer)?;
        if read_bytes == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read_bytes]);
    }
    let (byte_length, content_hash) = reader.finish();
    if byte_length == 0 {
        return Err(MediaParseError::invalid_srt("SRT 文件为空"));
    }

    let utf8 = std::str::from_utf8(&bytes).map_err(|_| MediaParseError::invalid_utf8())?;
    let without_bom = utf8.strip_prefix('\u{feff}').unwrap_or(utf8);
    let normalized = normalize_srt_text(without_bom)?;
    let cues = parse_srt_cues(&normalized, audio_duration_ms, limits)?;

    Ok(ParsedSrt {
        content_hash,
        byte_length,
        cues,
    })
}

fn validate_srt_limits(
    audio_duration_ms: u64,
    limits: SrtParseLimits,
) -> Result<(), MediaParseError> {
    if limits.max_bytes == 0 || limits.max_bytes > MAX_MEDIA_BYTES {
        return Err(MediaParseError::limit("SRT 读取上限必须位于支持范围内"));
    }
    if limits.max_cue_count == 0 || limits.max_cue_count > MAX_CAPTION_CUES {
        return Err(MediaParseError::limit("SRT cue 数量上限必须位于支持范围内"));
    }
    if limits.max_cue_text_bytes == 0 || limits.max_cue_text_bytes > MAX_CUE_TEXT_UTF8_BYTES {
        return Err(MediaParseError::limit("SRT cue 文本上限必须位于支持范围内"));
    }
    if audio_duration_ms == 0 || audio_duration_ms > MAX_AUDIO_DURATION_MS {
        return Err(MediaParseError::invalid_srt("音频时长超出支持范围"));
    }
    Ok(())
}

fn normalize_srt_text(source: &str) -> Result<String, MediaParseError> {
    let mut normalized = String::with_capacity(source.len());
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.next_if_eq(&'\n').is_none() {
                    return Err(MediaParseError::invalid_srt("SRT 包含孤立回车符"));
                }
                normalized.push('\n');
            }
            '\n' => normalized.push('\n'),
            value if is_unsafe_srt_character(value) => {
                return Err(MediaParseError::invalid_srt("SRT 包含不安全控制字符"));
            }
            value => normalized.push(value),
        }
    }
    if normalized.is_empty() {
        return Err(MediaParseError::invalid_srt("SRT 文件不包含 cue"));
    }
    if normalized.starts_with('\n') {
        return Err(MediaParseError::invalid_srt("SRT 不能以空白块开头"));
    }
    Ok(normalized)
}

fn is_unsafe_srt_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
}

fn parse_srt_cues(
    normalized: &str,
    audio_duration_ms: u64,
    limits: SrtParseLimits,
) -> Result<Vec<ParsedCaptionCue>, MediaParseError> {
    let mut cues = Vec::new();
    let mut block = Vec::new();
    for line in normalized.split('\n').chain(std::iter::once("")) {
        if !line.is_empty() && line.chars().all(char::is_whitespace) {
            return Err(MediaParseError::invalid_srt("SRT 包含仅由空白组成的行"));
        }
        if line.is_empty() {
            if !block.is_empty() {
                parse_srt_block(&block, &mut cues, audio_duration_ms, limits)?;
                block.clear();
            }
        } else {
            block.push(line);
        }
    }
    if cues.is_empty() {
        return Err(MediaParseError::invalid_srt("SRT 文件不包含 cue"));
    }
    Ok(cues)
}

fn parse_srt_block(
    block: &[&str],
    cues: &mut Vec<ParsedCaptionCue>,
    audio_duration_ms: u64,
    limits: SrtParseLimits,
) -> Result<(), MediaParseError> {
    if block.len() < 3 {
        return Err(MediaParseError::invalid_srt(
            "SRT cue 缺少序号、时间轴或文本",
        ));
    }
    if cues.len() >= limits.max_cue_count {
        return Err(MediaParseError::limit("SRT cue 数量超过上限"));
    }

    let expected_index = u32::try_from(cues.len() + 1).expect("cue schema limit fits u32");
    if block[0].is_empty()
        || !block[0].bytes().all(|byte| byte.is_ascii_digit())
        || block[0].parse::<u32>() != Ok(expected_index)
        || (block[0].len() > 1 && block[0].starts_with('0'))
    {
        return Err(MediaParseError::invalid_srt(
            "SRT cue 序号必须从 1 开始连续递增",
        ));
    }
    let (start_ms, end_ms) = parse_srt_timeline(block[1])?;
    if start_ms >= end_ms {
        return Err(MediaParseError::invalid_srt("SRT cue 必须满足 start < end"));
    }
    if end_ms > audio_duration_ms {
        return Err(MediaParseError::invalid_srt("SRT cue 超出音频时长"));
    }
    if let Some(previous) = cues.last() {
        if start_ms < previous.start_ms {
            return Err(MediaParseError::invalid_srt("SRT cue 时间顺序发生倒退"));
        }
        if start_ms < previous.end_ms {
            return Err(MediaParseError::invalid_srt("SRT cue 时间范围发生重叠"));
        }
    }

    let text = block[2..].join("\n");
    if !text.chars().any(|character| !character.is_whitespace()) {
        return Err(MediaParseError::invalid_srt("SRT cue 文本不能为空"));
    }
    if text.len() > limits.max_cue_text_bytes {
        return Err(MediaParseError::limit("SRT cue 文本超过 UTF-8 字节上限"));
    }
    if text.chars().count() > MAX_CUE_TEXT_CHARS {
        return Err(MediaParseError::limit("SRT cue 文本超过契约字符上限"));
    }

    cues.push(ParsedCaptionCue {
        cue_id: stable_cue_id(expected_index, start_ms, end_ms, &text),
        source_index: expected_index,
        start_ms,
        end_ms,
        text,
    });
    Ok(())
}

fn parse_srt_timeline(timeline: &str) -> Result<(u64, u64), MediaParseError> {
    let bytes = timeline.as_bytes();
    if bytes.len() != 29 || &bytes[12..17] != b" --> " {
        return Err(MediaParseError::invalid_srt("SRT 时间轴语法无效"));
    }
    let start_ms = parse_srt_timestamp(&bytes[..12])?;
    let end_ms = parse_srt_timestamp(&bytes[17..])?;
    Ok((start_ms, end_ms))
}

fn parse_srt_timestamp(bytes: &[u8]) -> Result<u64, MediaParseError> {
    if bytes.len() != 12
        || bytes[2] != b':'
        || bytes[5] != b':'
        || bytes[8] != b','
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 2 | 5 | 8) || byte.is_ascii_digit())
    {
        return Err(MediaParseError::invalid_srt("SRT 时间戳语法无效"));
    }
    let hours = parse_ascii_number(&bytes[0..2]);
    let minutes = parse_ascii_number(&bytes[3..5]);
    let seconds = parse_ascii_number(&bytes[6..8]);
    let milliseconds = parse_ascii_number(&bytes[9..12]);
    if minutes > 59 || seconds > 59 || milliseconds > 999 {
        return Err(MediaParseError::invalid_srt("SRT 时间戳数值超出范围"));
    }
    let total_ms = u64::from(hours)
        .checked_mul(3_600_000)
        .and_then(|value| value.checked_add(u64::from(minutes) * 60_000))
        .and_then(|value| value.checked_add(u64::from(seconds) * 1_000))
        .and_then(|value| value.checked_add(u64::from(milliseconds)))
        .ok_or_else(|| MediaParseError::invalid_srt("SRT 时间戳计算溢出"))?;
    if total_ms > MAX_AUDIO_DURATION_MS {
        return Err(MediaParseError::invalid_srt("SRT 时间戳超出契约范围"));
    }
    Ok(total_ms)
}

fn parse_ascii_number(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0_u32, |value, byte| value * 10 + u32::from(byte - b'0'))
}

fn stable_cue_id(source_index: u32, start_ms: u64, end_ms: u64, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"narracut:srt-cue:v1\0");
    hasher.update(source_index.to_le_bytes());
    hasher.update(start_ms.to_le_bytes());
    hasher.update(end_ms.to_le_bytes());
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    format!("cue_{}", &format_sha256(&digest)["sha256:".len()..])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{
        parse_pcm_wav_file, parse_srt_file, MediaParseErrorCode, PcmWavParseLimits, SrtParseLimits,
        MAX_AUDIO_DURATION_MS, MAX_MEDIA_BYTES,
    };

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            bytes.push(0);
        }
        bytes
    }

    fn pcm_format(
        audio_format: u16,
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        block_align: Option<u16>,
        byte_rate: Option<u32>,
    ) -> Vec<u8> {
        let expected_align = channels * (bits_per_sample / 8);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&audio_format.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(
            &byte_rate
                .unwrap_or(sample_rate * u32::from(expected_align))
                .to_le_bytes(),
        );
        bytes.extend_from_slice(&block_align.unwrap_or(expected_align).to_le_bytes());
        bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
        bytes
    }

    fn wave(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for value in chunks {
            bytes.extend_from_slice(value);
        }
        let riff_size = u32::try_from(bytes.len() - 8).expect("small test wave");
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes
    }

    fn parse_bytes(
        temp_dir: &TempDir,
        file_name: &str,
        bytes: &[u8],
        max_bytes: u64,
    ) -> Result<super::ParsedPcmWav, super::MediaParseError> {
        let path = temp_dir.path().join(file_name);
        fs::write(&path, bytes).expect("write fixture");
        parse_pcm_wav_file(&path, PcmWavParseLimits { max_bytes })
    }

    fn expected_hash(bytes: &[u8]) -> String {
        super::format_sha256(&Sha256::digest(bytes))
    }

    fn parse_srt_bytes(
        temp_dir: &TempDir,
        file_name: &str,
        bytes: &[u8],
        audio_duration_ms: u64,
        limits: SrtParseLimits,
    ) -> Result<super::ParsedSrt, super::MediaParseError> {
        let path = temp_dir.path().join(file_name);
        fs::write(&path, bytes).expect("write SRT fixture");
        parse_srt_file(&path, audio_duration_ms, limits)
    }

    fn default_srt_limits() -> SrtParseLimits {
        SrtParseLimits::default()
    }

    #[test]
    fn parses_pcm_wave_with_odd_unknown_chunk_and_hashes_every_byte() {
        let temp_dir = TempDir::new().expect("temp dir");
        let fmt = chunk(b"fmt ", &pcm_format(1, 1, 8_000, 8, None, None));
        let junk = chunk(b"JUNK", &[1, 2, 3]);
        let data = chunk(b"data", &[0; 16]);
        let bytes = wave(&[fmt, junk, data]);

        let parsed = parse_bytes(&temp_dir, "valid.wav", &bytes, bytes.len() as u64)
            .expect("valid PCM wave");

        assert_eq!(parsed.content_hash, expected_hash(&bytes));
        assert_eq!(parsed.byte_length, bytes.len() as u64);
        assert_eq!(parsed.duration_ms, 2);
        assert_eq!(parsed.channels, 1);
        assert_eq!(parsed.sample_rate, 8_000);
        assert_eq!(parsed.bits_per_sample, 8);
        assert_eq!(parsed.block_align, 1);
        assert_eq!(parsed.byte_rate, 8_000);
        assert_eq!(parsed.data_byte_length, 16);
    }

    #[test]
    fn accepts_all_contract_pcm_bit_depths() {
        let temp_dir = TempDir::new().expect("temp dir");
        for bits_per_sample in [8_u16, 16, 24, 32] {
            let format = pcm_format(1, 2, 8_000, bits_per_sample, None, None);
            let frame_bytes = usize::from(2 * (bits_per_sample / 8));
            let bytes = wave(&[
                chunk(b"fmt ", &format),
                chunk(b"data", &vec![0; frame_bytes * 8]),
            ]);
            let parsed = parse_bytes(
                &temp_dir,
                &format!("pcm-{bits_per_sample}.wav"),
                &bytes,
                bytes.len() as u64,
            )
            .expect("contract bit depth should parse");
            assert_eq!(parsed.bits_per_sample, bits_per_sample);
            assert_eq!(parsed.duration_ms, 1);
        }
    }

    #[test]
    fn rejects_bad_container_headers_and_compressed_audio() {
        let temp_dir = TempDir::new().expect("temp dir");
        let valid = wave(&[
            chunk(b"fmt ", &pcm_format(1, 1, 8_000, 16, None, None)),
            chunk(b"data", &[0; 16]),
        ]);
        let mut bad_riff = valid.clone();
        bad_riff[0..4].copy_from_slice(b"RIFX");
        let mut bad_wave = valid;
        bad_wave[8..12].copy_from_slice(b"AVI ");
        let compressed = wave(&[
            chunk(b"fmt ", &pcm_format(3, 1, 8_000, 16, None, None)),
            chunk(b"data", &[0; 16]),
        ]);

        for (name, bytes) in [
            ("bad-riff.wav", bad_riff),
            ("bad-wave.wav", bad_wave),
            ("compressed.wav", compressed),
        ] {
            let error = parse_bytes(&temp_dir, name, &bytes, bytes.len() as u64)
                .expect_err("invalid container must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidWav);
        }
    }

    #[test]
    fn rejects_duplicate_or_out_of_order_required_chunks() {
        let temp_dir = TempDir::new().expect("temp dir");
        let fmt = chunk(b"fmt ", &pcm_format(1, 1, 8_000, 16, None, None));
        let data = chunk(b"data", &[0; 16]);
        let cases = [
            ("data-first.wav", wave(&[data.clone(), fmt.clone()])),
            (
                "duplicate-fmt.wav",
                wave(&[fmt.clone(), fmt.clone(), data.clone()]),
            ),
            ("duplicate-data.wav", wave(&[fmt, data.clone(), data])),
        ];

        for (name, bytes) in cases {
            let error = parse_bytes(&temp_dir, name, &bytes, bytes.len() as u64)
                .expect_err("invalid chunk order must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidWav);
        }
    }

    #[test]
    fn rejects_forged_chunk_length_truncation_and_trailing_bytes() {
        let temp_dir = TempDir::new().expect("temp dir");
        let fmt = chunk(b"fmt ", &pcm_format(1, 1, 8_000, 16, None, None));
        let data = chunk(b"data", &[0; 16]);

        let mut forged = wave(&[fmt.clone(), data.clone()]);
        forged[16..20].copy_from_slice(&u32::MAX.to_le_bytes());

        let mut truncated = wave(&[fmt.clone(), data.clone()]);
        truncated.pop();

        let mut trailing = wave(&[fmt, data]);
        trailing.extend_from_slice(b"hidden");

        for (name, bytes) in [
            ("forged.wav", forged),
            ("truncated.wav", truncated),
            ("trailing.wav", trailing),
        ] {
            let error = parse_bytes(
                &temp_dir,
                name,
                &bytes,
                PcmWavParseLimits::default().max_bytes,
            )
            .expect_err("forged boundary must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidWav);
        }
    }

    #[test]
    fn rejects_invalid_pcm_alignment_rate_and_data_length() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bad_align = wave(&[
            chunk(b"fmt ", &pcm_format(1, 2, 8_000, 16, Some(2), None)),
            chunk(b"data", &[0; 16]),
        ]);
        let bad_rate = wave(&[
            chunk(b"fmt ", &pcm_format(1, 2, 8_000, 16, None, Some(1))),
            chunk(b"data", &[0; 16]),
        ]);
        let bad_data = wave(&[
            chunk(b"fmt ", &pcm_format(1, 2, 8_000, 16, None, None)),
            chunk(b"data", &[0; 3]),
        ]);
        let bad_sample_rate = wave(&[
            chunk(b"fmt ", &pcm_format(1, 1, 7_999, 16, None, None)),
            chunk(b"data", &[0; 16]),
        ]);

        for (name, bytes) in [
            ("bad-align.wav", bad_align),
            ("bad-rate.wav", bad_rate),
            ("bad-data.wav", bad_data),
            ("bad-sample-rate.wav", bad_sample_rate),
        ] {
            let error = parse_bytes(&temp_dir, name, &bytes, bytes.len() as u64)
                .expect_err("invalid PCM invariant must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidWav);
        }
    }

    #[test]
    fn enforces_limits_and_never_leaks_source_path() {
        let temp_dir = TempDir::new().expect("private temp dir");
        let bytes = wave(&[
            chunk(b"fmt ", &pcm_format(1, 1, 8_000, 16, None, None)),
            chunk(b"data", &[0; 16]),
        ]);

        let zero_limit =
            parse_bytes(&temp_dir, "secret-zero.wav", &bytes, 0).expect_err("zero limit must fail");
        assert_eq!(zero_limit.code, MediaParseErrorCode::ResourceLimitExceeded);

        let too_small = parse_bytes(
            &temp_dir,
            "secret-small.wav",
            &bytes,
            (bytes.len() - 1) as u64,
        )
        .expect_err("file over limit must fail");
        assert_eq!(too_small.code, MediaParseErrorCode::ResourceLimitExceeded);

        let missing_path = temp_dir.path().join("do-not-leak-secret.wav");
        let missing = super::parse_pcm_wav_file(&missing_path, PcmWavParseLimits::default())
            .expect_err("missing source must fail");
        for error in [zero_limit, too_small, missing] {
            let rendered = error.to_string();
            assert!(!rendered.contains("secret"));
            assert!(!rendered.contains(&temp_dir.path().display().to_string()));
        }
    }

    #[test]
    fn parses_bom_crlf_multiline_srt_and_hashes_raw_bytes() {
        let temp_dir = TempDir::new().expect("temp dir");
        let bytes = concat!(
            "\u{feff}1\r\n",
            "00:00:00,000 --> 00:00:01,000\r\n",
            "第一行\r\n",
            "第二行\r\n",
            "\r\n",
            "2\r\n",
            "00:00:01,000 --> 00:00:02,000\r\n",
            "边界相接\r\n"
        )
        .as_bytes();

        let parsed = parse_srt_bytes(
            &temp_dir,
            "captions.srt",
            bytes,
            2_000,
            default_srt_limits(),
        )
        .expect("valid BOM CRLF SRT");

        assert_eq!(parsed.content_hash, expected_hash(bytes));
        assert_eq!(parsed.byte_length, bytes.len() as u64);
        assert_eq!(parsed.cues.len(), 2);
        assert_eq!(parsed.cues[0].source_index, 1);
        assert_eq!(parsed.cues[0].start_ms, 0);
        assert_eq!(parsed.cues[0].end_ms, 1_000);
        assert_eq!(parsed.cues[0].text, "第一行\n第二行");
        assert_eq!(parsed.cues[1].start_ms, parsed.cues[0].end_ms);
        for cue in &parsed.cues {
            assert!(cue.cue_id.starts_with("cue_"));
            assert_eq!(cue.cue_id.len(), 68);
            assert!(cue.cue_id[4..].bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn cue_ids_are_stable_across_bom_and_line_ending_normalization() {
        let temp_dir = TempDir::new().expect("temp dir");
        let lf = b"1\n00:00:00,000 --> 00:00:01,000\nline one\nline two\n";
        let bom_crlf =
            b"\xef\xbb\xbf1\r\n00:00:00,000 --> 00:00:01,000\r\nline one\r\nline two\r\n";

        let first = parse_srt_bytes(&temp_dir, "lf.srt", lf, 1_000, default_srt_limits())
            .expect("valid LF SRT");
        let repeated = parse_srt_bytes(&temp_dir, "lf-repeat.srt", lf, 1_000, default_srt_limits())
            .expect("same SRT parses repeatedly");
        let normalized = parse_srt_bytes(
            &temp_dir,
            "bom-crlf.srt",
            bom_crlf,
            1_000,
            default_srt_limits(),
        )
        .expect("equivalent BOM CRLF SRT");

        assert_eq!(first.content_hash, repeated.content_hash);
        assert_ne!(first.content_hash, normalized.content_hash);
        assert_eq!(first.cues[0].cue_id, repeated.cues[0].cue_id);
        assert_eq!(first.cues[0].cue_id, normalized.cues[0].cue_id);
        assert_eq!(first.cues[0].text, normalized.cues[0].text);
    }

    #[test]
    fn rejects_non_utf8_isolated_cr_nul_and_directional_controls() {
        let temp_dir = TempDir::new().expect("temp dir");
        let invalid_utf8 = b"1\n00:00:00,000 --> 00:00:01,000\n\xff\n";
        let isolated_cr = b"1\r00:00:00,000 --> 00:00:01,000\ntext\n";
        let nul = b"1\n00:00:00,000 --> 00:00:01,000\ntext\0hidden\n";
        let directional = "1\n00:00:00,000 --> 00:00:01,000\nabc\u{202e}def\n".as_bytes();

        let utf8_error = parse_srt_bytes(
            &temp_dir,
            "invalid-utf8.srt",
            invalid_utf8,
            1_000,
            default_srt_limits(),
        )
        .expect_err("invalid UTF-8 must fail");
        assert_eq!(utf8_error.code, MediaParseErrorCode::InvalidUtf8);

        for (name, bytes) in [
            ("isolated-cr.srt", isolated_cr.as_slice()),
            ("nul.srt", nul.as_slice()),
            ("directional.srt", directional),
        ] {
            let error = parse_srt_bytes(&temp_dir, name, bytes, 1_000, default_srt_limits())
                .expect_err("unsafe control must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidSrt);
        }
    }

    #[test]
    fn requires_indices_to_start_at_one_and_increase_without_gaps() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cases = [
            (
                "starts-zero.srt",
                "0\n00:00:00,000 --> 00:00:01,000\nzero\n",
            ),
            (
                "leading-zero.srt",
                "01\n00:00:00,000 --> 00:00:01,000\nleading\n",
            ),
            (
                "gap.srt",
                concat!(
                    "1\n00:00:00,000 --> 00:00:01,000\none\n\n",
                    "3\n00:00:01,000 --> 00:00:02,000\nthree\n"
                ),
            ),
            (
                "repeat.srt",
                concat!(
                    "1\n00:00:00,000 --> 00:00:01,000\none\n\n",
                    "1\n00:00:01,000 --> 00:00:02,000\nrepeat\n"
                ),
            ),
        ];

        for (name, source) in cases {
            let error = parse_srt_bytes(
                &temp_dir,
                name,
                source.as_bytes(),
                2_000,
                default_srt_limits(),
            )
            .expect_err("non-contiguous index must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidSrt);
        }
    }

    #[test]
    fn rejects_invalid_timestamp_syntax_and_numeric_boundaries() {
        let temp_dir = TempDir::new().expect("temp dir");
        let timelines = [
            "00:00:00.000 --> 00:00:01,000",
            "0:00:00,000 --> 00:00:01,000",
            "00:60:00,000 --> 00:00:01,000",
            "00:00:60,000 --> 00:01:01,000",
            "25:00:00,000 --> 25:00:01,000",
            "00:00:00,000  --> 00:00:01,000",
            "00:00:00,000 --> 00:00:01,000 trailing",
            "00:00:00,000 —-> 00:00:01,000",
        ];

        for (index, timeline) in timelines.into_iter().enumerate() {
            let source = format!("1\n{timeline}\ntext\n");
            let error = parse_srt_bytes(
                &temp_dir,
                &format!("bad-timestamp-{index}.srt"),
                source.as_bytes(),
                MAX_AUDIO_DURATION_MS,
                default_srt_limits(),
            )
            .expect_err("invalid timestamp must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidSrt);
        }
    }

    #[test]
    fn accepts_exact_timestamp_ceiling() {
        let temp_dir = TempDir::new().expect("temp dir");
        let source = b"1\n23:59:59,999 --> 24:00:00,000\nlast millisecond\n";
        let parsed = parse_srt_bytes(
            &temp_dir,
            "timestamp-ceiling.srt",
            source,
            MAX_AUDIO_DURATION_MS,
            default_srt_limits(),
        )
        .expect("24-hour contract boundary should parse");
        assert_eq!(parsed.cues[0].start_ms, MAX_AUDIO_DURATION_MS - 1);
        assert_eq!(parsed.cues[0].end_ms, MAX_AUDIO_DURATION_MS);
    }

    #[test]
    fn rejects_invalid_ranges_order_overlap_and_audio_bounds() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cases = [
            (
                "equal.srt",
                "1\n00:00:01,000 --> 00:00:01,000\nequal\n",
                2_000,
            ),
            (
                "reverse.srt",
                "1\n00:00:02,000 --> 00:00:01,000\nreverse\n",
                2_000,
            ),
            (
                "out-of-order.srt",
                concat!(
                    "1\n00:00:02,000 --> 00:00:03,000\nfirst\n\n",
                    "2\n00:00:01,000 --> 00:00:01,500\nsecond\n"
                ),
                3_000,
            ),
            (
                "overlap.srt",
                concat!(
                    "1\n00:00:00,000 --> 00:00:01,000\nfirst\n\n",
                    "2\n00:00:00,999 --> 00:00:02,000\nsecond\n"
                ),
                2_000,
            ),
            (
                "past-audio.srt",
                "1\n00:00:00,000 --> 00:00:02,001\npast audio\n",
                2_000,
            ),
        ];

        for (name, source, audio_duration_ms) in cases {
            let error = parse_srt_bytes(
                &temp_dir,
                name,
                source.as_bytes(),
                audio_duration_ms,
                default_srt_limits(),
            )
            .expect_err("invalid cue range must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidSrt);
        }

        let valid = b"1\n00:00:00,000 --> 00:00:01,000\ntext\n";
        for invalid_duration in [0, MAX_AUDIO_DURATION_MS + 1] {
            let error = parse_srt_bytes(
                &temp_dir,
                &format!("duration-{invalid_duration}.srt"),
                valid,
                invalid_duration,
                default_srt_limits(),
            )
            .expect_err("invalid audio duration must fail");
            assert_eq!(error.code, MediaParseErrorCode::InvalidSrt);
        }
    }

    #[test]
    fn enforces_srt_byte_cue_and_text_limits_and_rejects_empty_text() {
        let temp_dir = TempDir::new().expect("temp dir");
        let one = b"1\n00:00:00,000 --> 00:00:01,000\nhello\n";
        let two = concat!(
            "1\n00:00:00,000 --> 00:00:01,000\none\n\n",
            "2\n00:00:01,000 --> 00:00:02,000\ntwo\n"
        )
        .as_bytes();
        let multibyte = "1\n00:00:00,000 --> 00:00:01,000\n你好\n".as_bytes();
        let empty = b"1\n00:00:00,000 --> 00:00:01,000\n\n";

        let cases = [
            (
                "total-bytes.srt",
                one.as_slice(),
                SrtParseLimits {
                    max_bytes: (one.len() - 1) as u64,
                    ..default_srt_limits()
                },
            ),
            (
                "cue-count.srt",
                two,
                SrtParseLimits {
                    max_cue_count: 1,
                    ..default_srt_limits()
                },
            ),
            (
                "cue-text.srt",
                multibyte,
                SrtParseLimits {
                    max_cue_text_bytes: 5,
                    ..default_srt_limits()
                },
            ),
        ];
        for (name, bytes, limits) in cases {
            let error = parse_srt_bytes(&temp_dir, name, bytes, 2_000, limits)
                .expect_err("configured resource limit must fail");
            assert_eq!(error.code, MediaParseErrorCode::ResourceLimitExceeded);
        }

        let empty_error = parse_srt_bytes(
            &temp_dir,
            "empty-text.srt",
            empty,
            1_000,
            default_srt_limits(),
        )
        .expect_err("empty cue text must fail");
        assert_eq!(empty_error.code, MediaParseErrorCode::InvalidSrt);

        let invalid_limits = [
            SrtParseLimits {
                max_bytes: 0,
                ..default_srt_limits()
            },
            SrtParseLimits {
                max_bytes: MAX_MEDIA_BYTES + 1,
                ..default_srt_limits()
            },
            SrtParseLimits {
                max_cue_count: 0,
                ..default_srt_limits()
            },
            SrtParseLimits {
                max_cue_text_bytes: 0,
                ..default_srt_limits()
            },
        ];
        for (index, limits) in invalid_limits.into_iter().enumerate() {
            let error = parse_srt_bytes(
                &temp_dir,
                &format!("invalid-limit-{index}.srt"),
                one,
                1_000,
                limits,
            )
            .expect_err("invalid limit configuration must fail");
            assert_eq!(error.code, MediaParseErrorCode::ResourceLimitExceeded);
        }
    }

    #[test]
    fn srt_errors_and_results_never_contain_source_paths() {
        let temp_dir = TempDir::new().expect("private temp dir");
        let secret_name = "private-project-secret.srt";
        let invalid = b"not an SRT";
        let error = parse_srt_bytes(&temp_dir, secret_name, invalid, 1_000, default_srt_limits())
            .expect_err("invalid SRT must fail");
        let missing_path = temp_dir.path().join("missing-secret.srt");
        let missing = parse_srt_file(&missing_path, 1_000, default_srt_limits())
            .expect_err("missing SRT must fail");
        for value in [error.to_string(), missing.to_string()] {
            assert!(!value.contains("secret"));
            assert!(!value.contains(&temp_dir.path().display().to_string()));
        }

        let valid = b"1\n00:00:00,000 --> 00:00:01,000\ntext\n";
        let result = parse_srt_bytes(&temp_dir, secret_name, valid, 1_000, default_srt_limits())
            .expect("valid SRT");
        let serialized = serde_json::to_string(&result).expect("serialize parsed SRT");
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains(&temp_dir.path().display().to_string()));
    }
}
