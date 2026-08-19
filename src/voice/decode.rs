//! Decode arbitrary audio files to the 16 kHz mono 16-bit WAV that whisper.cpp
//! accepts.
//!
//! whisper.cpp cannot read anything but 16-bit PCM WAV, and Discord delivers
//! voice messages as OGG/Opus and ordinary audio attachments as MP3, M4A, FLAC
//! and so on. Everything is decoded in-process:
//!
//! * OGG/Opus (Discord voice messages) goes through `ogg` + `opus`, which
//!   compile libopus from source.
//! * Everything else (WAV, MP3, FLAC, AAC/M4A, OGG/Vorbis, WebM) goes through
//!   [`symphonia`].
//!
//! Output is always 16-bit mono at 16 kHz, matching what the web UI's recorder
//! already feeds the transcribe endpoint.

use std::io::Write;
use std::path::Path;

/// The sample rate whisper.cpp expects. The web recorder resamples to this
/// before sending, so keeping Discord input identical removes one variable.
const TARGET_RATE: u32 = 16_000;

/// Convert `input` into a 16-bit mono 16 kHz WAV written to `output`.
pub fn decode_audio_to_wav(input: &Path, output: &Path) -> Result<(), String> {
    let data = std::fs::read(input)
        .map_err(|e| format!("Could not read audio file {}: {}", input.display(), e))?;

    let (pcm, rate) = if looks_like_opus(&data) {
        decode_ogg_opus(&data)?
    } else {
        decode_with_symphonia(input)?
    };

    let resampled = resample_mono(&pcm, rate);
    write_wav(output, &resampled, TARGET_RATE)
}

/// Opus lives in an OGG container that symphonia does not support, so sniff for
/// the `OggS` magic plus an `OpusHead` packet before falling back to symphonia
/// (which handles OGG/Vorbis fine).
fn looks_like_opus(data: &[u8]) -> bool {
    data.starts_with(b"OggS") && data.windows(8).any(|window| window == b"OpusHead")
}

// ---------------------------------------------------------------------------
// OGG/Opus
// ---------------------------------------------------------------------------

/// Decode an OGG/Opus stream to mono PCM at 48 kHz.
fn decode_ogg_opus(data: &[u8]) -> Result<(Vec<i16>, u32), String> {
    use std::io::Cursor;

    let mut reader = ogg::reading::PacketReader::new(Cursor::new(data.to_vec()));
    let mut decoder: Option<opus::Decoder> = None;
    let mut channels = 0usize;
    let mut mono_pcm: Vec<i16> = Vec::new();

    while let Some(packet) = reader
        .read_packet()
        .map_err(|e| format!("Could not read OGG packet: {e}"))?
    {
        if packet.data.starts_with(b"OpusHead") {
            // Byte 9 of OpusHead is the channel count. Voice messages are mono
            // or stereo; anything larger is rejected rather than guessed at.
            let count = packet.data.get(9).copied().unwrap_or(1);
            channels = count as usize;
            decoder = Some(
                match count {
                    1 => opus::Decoder::new(48_000, opus::Channels::Mono),
                    2 => opus::Decoder::new(48_000, opus::Channels::Stereo),
                    n => return Err(format!("Unsupported Opus channel count: {n}")),
                }
                .map_err(|e| format!("Could not initialise Opus decoder: {e}"))?,
            );
            continue;
        }
        if packet.data.starts_with(b"OpusTags") {
            continue;
        }
        let Some(decoder) = decoder.as_mut() else {
            continue;
        };

        // An Opus frame is at most 120 ms at 48 kHz = 5760 samples per channel.
        let mut out = vec![0i16; 5760 * channels];
        let samples_per_channel = decoder
            .decode(&packet.data, &mut out, false)
            .map_err(|e| format!("Could not decode Opus packet: {e}"))?;
        let total = samples_per_channel * channels;

        // Downmix every channel pair into a single mono sample.
        for frame in out[..total].chunks_exact(channels) {
            let sum: i32 = frame.iter().map(|&sample| sample as i32).sum();
            mono_pcm.push((sum / channels as i32) as i16);
        }
    }

    if mono_pcm.is_empty() {
        return Err("The OGG/Opus file contained no audio.".to_string());
    }
    Ok((mono_pcm, 48_000))
}

// ---------------------------------------------------------------------------
// symphonia (everything else)
// ---------------------------------------------------------------------------

/// Decode any symphonia-supported file to mono PCM, returned as f32 samples at
/// their native rate (converted to i16 once, after downmixing).
fn decode_with_symphonia(input: &Path) -> Result<(Vec<i16>, u32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(input)
        .map_err(|e| format!("Could not open audio file {}: {}", input.display(), e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = input.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("Unsupported audio format: {e}"))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "The audio file has no decodable track.".to_string())?
        .clone();
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Could not create audio decoder: {e}"))?;

    let mut mono_f32: Vec<f32> = Vec::new();
    let mut sample_rate = 0u32;
    let mut channel_count: u32;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // End of stream and an occasional corrupt trailing packet are both
            // normal outcomes for real-world attachments.
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Failed reading audio: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(buffer) => {
                let spec = *buffer.spec();
                sample_rate = spec.rate;
                channel_count = spec.channels.count() as u32;

                let mut interleaved = SampleBuffer::<f32>::new(buffer.capacity() as u64, spec);
                interleaved.copy_interleaved_ref(buffer);

                for frame in interleaved.samples().chunks_exact(channel_count as usize) {
                    let sum: f32 = frame.iter().sum();
                    mono_f32.push(sum / channel_count as f32);
                }
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Failed decoding audio: {e}")),
        }
    }

    if mono_f32.is_empty() {
        return Err("The audio file contained no audio.".to_string());
    }

    let pcm: Vec<i16> = mono_f32
        .iter()
        .map(|&sample| (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect();
    Ok((pcm, sample_rate))
}

// ---------------------------------------------------------------------------
// resampling and WAV output
// ---------------------------------------------------------------------------

/// Linear-interpolation resampler to [`TARGET_RATE`]. Good enough for speech,
/// and keeps whisper input uniform regardless of the source sample rate.
fn resample_mono(samples: &[i16], from_rate: u32) -> Vec<i16> {
    if from_rate == TARGET_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let step = from_rate as f64 / TARGET_RATE as f64;
    let mut out = Vec::with_capacity((samples.len() as f64 / step) as usize + 1);
    let mut position = 0.0f64;
    while position as usize + 1 < samples.len() {
        let index = position as usize;
        let fraction = position - index as f64;
        let a = samples[index] as f64;
        let b = samples[index + 1] as f64;
        out.push((a + (b - a) * fraction).round() as i16);
        position += step;
    }
    out
}

/// Write 16-bit mono PCM as a minimal RIFF/WAVE file.
fn write_wav(path: &Path, samples: &[i16], rate: u32) -> Result<(), String> {
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("Could not write {}: {}", path.display(), e))?;
    let data_len = (samples.len() * 2) as u32;

    let write_all = |file: &mut std::fs::File, bytes: &[u8]| -> Result<(), String> {
        file.write_all(bytes)
            .map_err(|e| format!("Failed writing WAV: {e}"))
    };

    write_all(&mut file, b"RIFF")?;
    write_all(&mut file, &(36 + data_len).to_le_bytes())?;
    write_all(&mut file, b"WAVE")?;
    write_all(&mut file, b"fmt ")?;
    write_all(&mut file, &16u32.to_le_bytes())?; // fmt chunk size
    write_all(&mut file, &1u16.to_le_bytes())?; // PCM
    write_all(&mut file, &1u16.to_le_bytes())?; // mono
    write_all(&mut file, &rate.to_le_bytes())?;
    write_all(&mut file, &(rate * 2).to_le_bytes())?; // byte rate
    write_all(&mut file, &2u16.to_le_bytes())?; // block align
    write_all(&mut file, &16u16.to_le_bytes())?; // bits per sample
    write_all(&mut file, b"data")?;
    write_all(&mut file, &data_len.to_le_bytes())?;

    for sample in samples {
        write_all(&mut file, &sample.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogg::PacketWriteEndInfo;
    use std::io::Cursor;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("osa_decode_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    fn write_wav_to_disk(path: &std::path::Path, samples: &[i16], rate: u32) {
        write_wav(path, samples, rate).unwrap();
    }

    /// Build a minimal but valid OGG/Opus file from PCM so the decoder is
    /// tested against the exact format Discord voice messages use.
    fn encode_opus_ogg(pcm: &[i16], channels: opus::Channels) -> Vec<u8> {
        let channel_count = match channels {
            opus::Channels::Mono => 1,
            opus::Channels::Stereo => 2,
        };
        let mut out: Vec<u8> = Vec::new();
        let mut writer = ogg::writing::PacketWriter::new(&mut out);
        let serial = 1u32;

        let mut head = Vec::new();
        head.extend_from_slice(b"OpusHead");
        head.push(1u8);
        head.push(channel_count as u8);
        head.extend_from_slice(&0u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0u8);
        writer
            .write_packet(head, serial, PacketWriteEndInfo::NormalPacket, 0)
            .unwrap();

        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&6u32.to_le_bytes());
        tags.extend_from_slice(b"osatest");
        tags.extend_from_slice(&0u32.to_le_bytes());
        writer
            .write_packet(tags, serial, PacketWriteEndInfo::NormalPacket, 0)
            .unwrap();

        let mut encoder = opus::Encoder::new(48_000, channels, opus::Application::Voip).unwrap();
        let frame = 960; // 20 ms at 48 kHz
        let mut granule = 0u64;
        for chunk in pcm.chunks(frame * channel_count) {
            let mut padded = chunk.to_vec();
            padded.resize(frame * channel_count, 0);
            granule += frame as u64;
            let packet = encoder.encode_vec(&padded, 4000).unwrap();
            writer
                .write_packet(packet, serial, PacketWriteEndInfo::NormalPacket, granule)
                .unwrap();
        }
        writer
            .write_packet(
                Vec::<u8>::new(),
                serial,
                PacketWriteEndInfo::EndStream,
                granule,
            )
            .unwrap();
        out
    }

    fn read_wav(path: &std::path::Path) -> (u16, u32, u16, u32) {
        let bytes = std::fs::read(path).unwrap();
        let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
        let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
        let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
        let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        (channels, rate, bits, data_len)
    }

    #[test]
    fn opus_roundtrip_produces_16k_mono_wav() {
        let pcm: Vec<i16> = (0..48_000)
            .map(|i| {
                ((i as f64 * 2.0 * std::f64::consts::PI * 440.0 / 48_000.0).sin() * 12_000.0) as i16
            })
            .collect();
        let ogg = encode_opus_ogg(&pcm, opus::Channels::Mono);
        assert!(ogg.starts_with(b"OggS"), "encoded data must be OGG");

        let input = temp_file("voice.ogg");
        let output = temp_file("voice.wav");
        std::fs::write(&input, &ogg).unwrap();
        decode_audio_to_wav(&input, &output).unwrap();

        let (channels, rate, bits, data_len) = read_wav(&output);
        assert_eq!(channels, 1);
        assert_eq!(rate, 16_000);
        assert_eq!(bits, 16);
        assert!(data_len > 0);
        assert_eq!(
            data_len as usize + 44,
            std::fs::metadata(&output).unwrap().len() as usize
        );
    }

    #[test]
    fn stereo_opus_is_downmixed_to_mono() {
        let stereo: Vec<i16> = (0..48_000)
            .flat_map(|i| {
                let v = ((i as f64 * 2.0 * std::f64::consts::PI * 220.0 / 48_000.0).sin() * 8_000.0)
                    as i16;
                vec![v, v]
            })
            .collect();
        let ogg = encode_opus_ogg(&stereo, opus::Channels::Stereo);
        let input = temp_file("stereo.ogg");
        let output = temp_file("stereo.wav");
        std::fs::write(&input, &ogg).unwrap();
        decode_audio_to_wav(&input, &output).unwrap();
        assert_eq!(read_wav(&output).0, 1, "stereo must be downmixed to mono");
    }

    #[test]
    fn wav_roundtrip_through_symphonia() {
        let pcm: Vec<i16> = (0..16_000)
            .map(|i| {
                ((i as f64 * 2.0 * std::f64::consts::PI * 330.0 / 16_000.0).sin() * 9_000.0) as i16
            })
            .collect();
        let input = temp_file("src.wav");
        let output = temp_file("out.wav");
        write_wav_to_disk(&input, &pcm, 16_000);
        decode_audio_to_wav(&input, &output).unwrap();
        assert_eq!(read_wav(&output).1, 16_000);
    }

    #[test]
    fn resample_48k_to_16k_keeps_expected_length() {
        let pcm: Vec<i16> = (0..48_000).map(|_| 0i16).collect();
        let resampled = resample_mono(&pcm, 48_000);
        let expected = (pcm.len() as f64 * 16_000.0 / 48_000.0).round() as usize;
        assert!(
            (resampled.len() as i64 - expected as i64).abs() <= 1,
            "unexpected resampled length {} vs {expected}",
            resampled.len()
        );
    }

    #[test]
    fn garbage_is_rejected_not_panicked_on() {
        let input = temp_file("garbage.bin");
        let output = temp_file("garbage.wav");
        std::fs::write(&input, b"definitely not audio").unwrap();
        assert!(decode_audio_to_wav(&input, &output).is_err());
    }

    #[test]
    fn resample_is_identity_at_target_rate() {
        let pcm = vec![1i16, 2, 3, 4, 5];
        assert_eq!(resample_mono(&pcm, TARGET_RATE), pcm);
    }

    #[test]
    fn non_opus_ogg_falls_through_to_symphonia() {
        // A corrupt OGG must not be mistaken for Opus: it should go to
        // symphonia and produce a decode error, not a panic.
        let input = temp_file("vorbis.ogg");
        let output = temp_file("vorbis.wav");
        std::fs::write(&input, b"OggS").unwrap();
        assert!(decode_audio_to_wav(&input, &output).is_err());
    }

    #[test]
    fn empty_cursor_read_is_safe() {
        let _ = Cursor::new(Vec::<u8>::new());
    }
}
