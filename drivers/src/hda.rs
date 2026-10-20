// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Intel High Definition Audio (High Definition Audio Specification 1.0a;
//! `docs/sound.md`): one output stream, 48 kHz, 16-bit, stereo, to the
//! first codec's first wired output pin.
//!
//! ```text
//!  BAR0: GCAP 0x00 (input/output stream counts) GCTL 0x08 (CRST)
//!        STATESTS 0x0E (codecs present) ICOI 0x60 ICII 0x64 ICIS 0x68
//!        stream descriptors from 0x80, 0x20 each, inputs first:
//!        CTL +0 (SRST, RUN; stream tag in +2 bits 7:4) STS +3 LPIB +4
//!        CBL +8 LVI +0xC FMT +0x12 BDLPL +0x18 BDLPU +0x1C
//!  DMA: the buffer descriptor list (2 entries) and a 64 KiB cyclic buffer
//!       of two 32 KiB halves
//! ```
//!
//! Codec commands go through the immediate command registers (ICOI/ICII/
//! ICIS, spec 3.4.3): 68k_HAL found QEMU's controller answering only the
//! first command sent through the CORB/RIRB rings.

/// Bytes of the cyclic DMA buffer.
pub const RING_BYTES: usize = 64 * 1024;
/// Stream format: 48 kHz base, 16 bits, 2 channels (spec 3.7.1).
pub const STREAM_FORMAT: u16 = 0x0011;
/// The stream tag the converter listens to.
pub const STREAM_TAG: u32 = 1;

/// Verbs (spec 7.3.3).
pub mod verb {
    /// Get parameter.
    pub const GET_PARAMETER: u32 = 0xF00;
    /// Get connection list entries.
    pub const GET_CONNECTION_LIST: u32 = 0xF02;
    /// Get configuration default (pins).
    pub const GET_CONFIG_DEFAULT: u32 = 0xF1C;
    /// Select the connection (input) in use.
    pub const SET_CONNECTION_SELECT: u32 = 0x701;
    /// Set power state.
    pub const SET_POWER_STATE: u32 = 0x705;
    /// Set the converter's stream and channel.
    pub const SET_STREAM_CHANNEL: u32 = 0x706;
    /// Set pin widget control.
    pub const SET_PIN_CONTROL: u32 = 0x707;
    /// Set EAPD/BTL enable.
    pub const SET_EAPD: u32 = 0x70C;
    /// Set the converter format (4-bit verb, 16-bit payload).
    pub const SET_FORMAT: u32 = 0x2;
    /// Set amplifier gain/mute (4-bit verb, 16-bit payload).
    pub const SET_AMP: u32 = 0x3;
}

/// Parameters (spec 7.3.4).
pub mod param {
    /// Subordinate node count: start in bits 23:16, count in 7:0.
    pub const NODE_COUNT: u32 = 0x04;
    /// Function group type (1 = audio).
    pub const FUNCTION_TYPE: u32 = 0x05;
    /// Audio widget capabilities: type in bits 23:20.
    pub const WIDGET_CAPS: u32 = 0x09;
    /// Connection list length: count in 6:0, long form in bit 7.
    pub const CONNECTION_LENGTH: u32 = 0x0E;
    /// Output amplifier capabilities: offset (0 dB step) in 6:0.
    pub const AMP_CAPS_OUT: u32 = 0x12;
}

/// Widget types (spec 7.3.4.6).
pub mod widget {
    /// Audio output (a DAC).
    pub const AUDIO_OUTPUT: u32 = 0x0;
    /// Mixer.
    pub const MIXER: u32 = 0x2;
    /// Selector.
    pub const SELECTOR: u32 = 0x3;
    /// Pin complex.
    pub const PIN: u32 = 0x4;
}

/// A 32-bit codec command: codec address, node id, verb and payload (a
/// 12-bit verb with 8 bits, or a 4-bit verb with 16 bits; spec 7.3).
pub fn make_verb(codec: u32, nid: u32, verb: u32, payload: u32) -> u32 {
    if verb < 0x10 {
        codec << 28 | nid << 20 | verb << 16 | (payload & 0xFFFF)
    } else {
        codec << 28 | nid << 20 | verb << 8 | (payload & 0xFF)
    }
}

/// Talking to a codec: one verb, one response.
pub trait Codec {
    /// Sends `verb` with `payload` to node `nid`; the response (0 if
    /// there was none).
    fn send(&mut self, nid: u32, verb: u32, payload: u32) -> u32;
}

/// The nodes of an output path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Path {
    /// The audio function group.
    pub afg: u32,
    /// The converter.
    pub dac: u32,
    /// A mixer or selector between them (0: none).
    pub middle: u32,
    /// The output pin.
    pub pin: u32,
    /// The pin's connection index that leads to `dac` or `middle`.
    pub pin_input: u32,
}

fn widget_type(c: &mut dyn Codec, nid: u32) -> u32 {
    (c.send(nid, verb::GET_PARAMETER, param::WIDGET_CAPS) >> 20) & 0xF
}

/// The inputs of a widget (short or long form connection list, spec
/// 7.3.3.3).
pub fn connections(c: &mut dyn Codec, nid: u32) -> alloc::vec::Vec<u32> {
    let info = c.send(nid, verb::GET_PARAMETER, param::CONNECTION_LENGTH);
    let long = info & 0x80 != 0;
    let count = info & 0x7F;
    let per = if long { 2 } else { 4 };
    (0..count)
        .map(|i| {
            let r = c.send(nid, verb::GET_CONNECTION_LIST, (i / per) * per);
            let shift = (i % per) * if long { 16 } else { 8 };
            (r >> shift) & if long { 0x7FFF } else { 0x7F }
        })
        .collect()
}

/// The first output path: an audio function group, a pin wired as line
/// out, speaker or headphone (configuration default: connectivity not
/// "none", device 0-2), and an audio output reached directly or through
/// one mixer or selector.
pub fn find_output(c: &mut dyn Codec) -> Option<Path> {
    let groups = c.send(0, verb::GET_PARAMETER, param::NODE_COUNT);
    let (first_group, group_count) = ((groups >> 16) & 0xFF, groups & 0xFF);
    for afg in first_group..first_group + group_count {
        if c.send(afg, verb::GET_PARAMETER, param::FUNCTION_TYPE) & 0xFF != 1 {
            continue;
        }
        let widgets = c.send(afg, verb::GET_PARAMETER, param::NODE_COUNT);
        let (first, count) = ((widgets >> 16) & 0xFF, widgets & 0xFF);
        for pin in first..first + count {
            if widget_type(c, pin) != widget::PIN {
                continue;
            }
            let config = c.send(pin, verb::GET_CONFIG_DEFAULT, 0);
            let (connectivity, device) = (config >> 30, (config >> 20) & 0xF);
            if connectivity == 1 || device > 2 {
                continue;
            }
            for (i, source) in connections(c, pin).into_iter().enumerate() {
                match widget_type(c, source) {
                    widget::AUDIO_OUTPUT => {
                        return Some(Path {
                            afg,
                            dac: source,
                            middle: 0,
                            pin,
                            pin_input: i as u32,
                        });
                    }
                    widget::MIXER | widget::SELECTOR => {
                        for inner in connections(c, source) {
                            if widget_type(c, inner) == widget::AUDIO_OUTPUT {
                                return Some(Path {
                                    afg,
                                    dac: inner,
                                    middle: source,
                                    pin,
                                    pin_input: i as u32,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

/// Unmutes a widget's output amplifier and its input amplifiers at 0 dB
/// (spec 7.3.3.7: bit 15 output, 14 input, 13/12 left/right, 11:8 index).
fn unmute(c: &mut dyn Codec, afg: u32, nid: u32) {
    let caps = c.send(nid, verb::GET_PARAMETER, param::WIDGET_CAPS);
    let amp_from = if caps & 0x8 != 0 { nid } else { afg };
    let zero_db = c.send(amp_from, verb::GET_PARAMETER, param::AMP_CAPS_OUT) & 0x7F;
    if caps & 0x4 != 0 {
        c.send(nid, verb::SET_AMP, 0xB000 | zero_db);
    }
    if caps & 0x2 != 0 {
        let inputs = connections(c, nid).len().clamp(1, 16) as u32;
        for i in 0..inputs {
            c.send(nid, verb::SET_AMP, 0x7000 | i << 8 | zero_db);
        }
    }
}

/// Powers up and unmutes the path, points the converter at our stream
/// (tag 1, channel 0, our format) and enables the pin's output and EAPD.
pub fn prepare(c: &mut dyn Codec, path: &Path) {
    c.send(path.afg, verb::SET_POWER_STATE, 0);
    for nid in [path.dac, path.middle, path.pin] {
        if nid != 0 {
            c.send(nid, verb::SET_POWER_STATE, 0);
            unmute(c, path.afg, nid);
        }
    }
    c.send(path.pin, verb::SET_CONNECTION_SELECT, path.pin_input);
    c.send(path.dac, verb::SET_STREAM_CHANNEL, STREAM_TAG << 4);
    c.send(path.dac, verb::SET_FORMAT, u32::from(STREAM_FORMAT));
    c.send(path.pin, verb::SET_PIN_CONTROL, 0x40);
    c.send(path.pin, verb::SET_EAPD, 0x02);
}

/// A buffer descriptor list entry (spec 3.6.3): address, length, IOC.
pub fn bdl_entry(addr: u64, len: u32, ioc: bool) -> [u8; 16] {
    let mut e = [0u8; 16];
    e[0..8].copy_from_slice(&addr.to_le_bytes());
    e[8..12].copy_from_slice(&len.to_le_bytes());
    e[12] = u8::from(ioc);
    e
}

#[cfg(target_os = "none")]
pub use system::{Hda, probe};

#[cfg(target_os = "none")]
mod system {
    use super::*;
    use crate::pci::system as pci;
    use crate::sync::spin_for;
    use frame::mm::dma::DmaBuffer;
    use frame::mm::mmio::MmioRegion;

    const GCAP: usize = 0x00;
    const GCTL: usize = 0x08;
    const STATESTS: usize = 0x0E;
    const ICOI: usize = 0x60;
    const ICII: usize = 0x64;
    const ICIS: usize = 0x68;
    const STREAMS: usize = 0x80;
    const SD_CTL: usize = 0x00;
    const SD_STS: usize = 0x03;
    const SD_LPIB: usize = 0x04;
    const SD_CBL: usize = 0x08;
    const SD_LVI: usize = 0x0C;
    const SD_FMT: usize = 0x12;
    const SD_BDLPL: usize = 0x18;
    const SD_BDLPU: usize = 0x1C;

    /// The controller with its output stream.
    pub struct Hda {
        regs: MmioRegion,
        codec: u32,
        stream: usize,
        bdl: DmaBuffer,
        ring: DmaBuffer,
    }

    struct Immediate<'a> {
        regs: &'a MmioRegion,
        codec: u32,
    }

    impl Codec for Immediate<'_> {
        /// Spec 3.4.3: wait until ICB (bit 0) is clear, write ICOI, set
        /// ICB, wait for IRV (bit 1), read ICII, clear IRV (write 1).
        fn send(&mut self, nid: u32, verb: u32, payload: u32) -> u32 {
            let r = self.regs;
            if !spin_for(10_000_000, || r.read_u16(ICIS) & 1 == 0) {
                return 0;
            }
            r.write_u16(ICIS, 0x2);
            r.write_u32(ICOI, make_verb(self.codec, nid, verb, payload));
            r.write_u16(ICIS, 0x1);
            if !spin_for(10_000_000, || r.read_u16(ICIS) & 0x3 == 0x2) {
                return 0;
            }
            let v = r.read_u32(ICII);
            r.write_u16(ICIS, 0x2);
            v
        }
    }

    impl Hda {
        fn sd(&self, reg: usize) -> usize {
            self.stream + reg
        }

        /// Starts the stream (DMA runs over the cyclic buffer).
        pub fn start(&self) {
            let ctl = self.sd(SD_CTL);
            self.regs.write_u8(ctl, self.regs.read_u8(ctl) | 0x2);
        }

        /// Stops it.
        pub fn stop(&self) {
            let ctl = self.sd(SD_CTL);
            self.regs.write_u8(ctl, self.regs.read_u8(ctl) & !0x2);
            self.regs.write_u8(self.sd(SD_STS), 0x1C);
        }

        /// The byte the controller reads next (link position in buffer).
        pub fn position(&self) -> usize {
            self.regs.read_u32(self.sd(SD_LPIB)) as usize % RING_BYTES
        }

        /// Copies `data` into the cyclic buffer at `offset` (wrapping).
        pub fn write(&self, offset: usize, data: &[u8]) {
            let at = offset % RING_BYTES;
            let first = data.len().min(RING_BYTES - at);
            let _ = self.ring.write(at, &data[..first]);
            if first < data.len() {
                let _ = self.ring.write(0, &data[first..]);
            }
        }

        /// Fills the cyclic buffer with silence.
        pub fn silence(&self) {
            self.ring.zero();
        }
    }

    /// The first HDA controller (PCI class 04/03) with an output path,
    /// its stream programmed but stopped.
    pub fn probe() -> Option<Hda> {
        let f = pci::devices()
            .iter()
            .find(|f| f.class == 0x04 && f.subclass == 0x03)?;
        match init(f) {
            Ok(h) => Some(h),
            Err(why) => {
                frame::kprintln!("hda: {}: {why}", f.address);
                None
            }
        }
    }

    fn init(f: &crate::pci::Function) -> Result<Hda, &'static str> {
        pci::enable_bus_master(f);
        let regs = pci::map_bar(f, 0).map_err(|_| "cannot map BAR0")?;
        // Controller reset: CRST 0, then 1 (spec 4.2.2); the codecs
        // announce themselves in STATESTS within 521 us of the reset.
        regs.write_u32(GCTL, regs.read_u32(GCTL) & !1);
        if !spin_for(100_000_000, || regs.read_u32(GCTL) & 1 == 0) {
            return Err("reset did not start");
        }
        regs.write_u32(GCTL, regs.read_u32(GCTL) | 1);
        if !spin_for(100_000_000, || regs.read_u32(GCTL) & 1 == 1) {
            return Err("reset did not end");
        }
        spin_for(2_000_000, || false);
        let present = regs.read_u16(STATESTS);
        if present & 0x7FFF == 0 {
            return Err("no codec");
        }
        let codec = present.trailing_zeros();
        let gcap = regs.read_u16(GCAP);
        let (outputs, inputs) = (u32::from(gcap >> 12) & 0xF, usize::from(gcap >> 8) & 0xF);
        if outputs == 0 {
            return Err("no output stream");
        }
        let path = {
            let mut c = Immediate { regs: &regs, codec };
            let path = find_output(&mut c).ok_or("no output path in the codec")?;
            prepare(&mut c, &path);
            path
        };
        let bdl = DmaBuffer::new(1).ok_or("no DMA memory")?;
        let ring = DmaBuffer::new(RING_BYTES / 4096).ok_or("no DMA memory")?;
        let stream = STREAMS + inputs * 0x20;
        let h = Hda {
            regs,
            codec,
            stream,
            bdl,
            ring,
        };
        // Stream reset: SRST 1, wait, 0, wait (spec 3.3.35).
        let ctl = h.sd(SD_CTL);
        h.regs.write_u8(ctl, 0x1);
        if !spin_for(10_000_000, || h.regs.read_u8(ctl) & 1 == 1) {
            return Err("stream reset did not start");
        }
        h.regs.write_u8(ctl, 0x0);
        if !spin_for(10_000_000, || h.regs.read_u8(ctl) & 1 == 0) {
            return Err("stream reset did not end");
        }
        let half = (RING_BYTES / 2) as u32;
        let _ = h.bdl.write(0, &bdl_entry(h.ring.phys(), half, true));
        let _ = h
            .bdl
            .write(16, &bdl_entry(h.ring.phys() + u64::from(half), half, true));
        h.regs.write_u8(h.sd(SD_CTL + 2), (STREAM_TAG << 4) as u8);
        h.regs.write_u32(h.sd(SD_CBL), RING_BYTES as u32);
        h.regs.write_u16(h.sd(SD_LVI), 1);
        h.regs.write_u16(h.sd(SD_FMT), STREAM_FORMAT);
        h.regs.write_u32(h.sd(SD_BDLPL), h.bdl.phys() as u32);
        h.regs
            .write_u32(h.sd(SD_BDLPU), (h.bdl.phys() >> 32) as u32);
        frame::kprintln!(
            "hda: {}: codec {}, output pin {} from converter {}{}, stream at {:#x}",
            f.address,
            h.codec,
            path.pin,
            path.dac,
            if path.middle != 0 {
                " through a mixer"
            } else {
                ""
            },
            stream
        );
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A codec described by its parameters and connection lists.
    #[derive(Default)]
    struct Fake {
        params: HashMap<(u32, u32), u32>,
        lists: HashMap<u32, Vec<u32>>,
        configs: HashMap<u32, u32>,
        sent: Vec<(u32, u32, u32)>,
    }

    impl Fake {
        fn widget(&mut self, nid: u32, kind: u32, caps: u32, inputs: &[u32]) {
            self.params
                .insert((nid, param::WIDGET_CAPS), kind << 20 | caps);
            self.params
                .insert((nid, param::CONNECTION_LENGTH), inputs.len() as u32);
            self.lists.insert(nid, inputs.to_vec());
        }
    }

    impl Codec for Fake {
        fn send(&mut self, nid: u32, verb: u32, payload: u32) -> u32 {
            match verb {
                verb::GET_PARAMETER => *self.params.get(&(nid, payload)).unwrap_or(&0),
                verb::GET_CONFIG_DEFAULT => *self.configs.get(&nid).unwrap_or(&0),
                verb::GET_CONNECTION_LIST => {
                    let list = self.lists.get(&nid).cloned().unwrap_or_default();
                    (0..4).fold(0, |acc, k| {
                        acc | list.get((payload + k) as usize).copied().unwrap_or(0) << (8 * k)
                    })
                }
                _ => {
                    self.sent.push((nid, verb, payload));
                    0
                }
            }
        }
    }

    /// Root 0 → function group 1 (audio) → widgets 2..=6: DAC 2, mixer
    /// 3 (inputs 2), an unconnected pin 4, a line-in pin 5, a headphone
    /// pin 6 (inputs 5, 3).
    fn codec() -> Fake {
        let mut c = Fake::default();
        c.params.insert((0, param::NODE_COUNT), 1 << 16 | 1);
        c.params.insert((1, param::FUNCTION_TYPE), 1);
        c.params.insert((1, param::NODE_COUNT), 2 << 16 | 5);
        c.params.insert((1, param::AMP_CAPS_OUT), 0x27);
        c.widget(2, widget::AUDIO_OUTPUT, 0x4, &[]);
        c.widget(3, widget::MIXER, 0x2, &[2]);
        c.widget(4, widget::PIN, 0, &[2]);
        c.widget(5, widget::PIN, 0, &[]);
        c.widget(6, widget::PIN, 0, &[5, 3]);
        c.configs.insert(4, 1 << 30); // no physical connection
        c.configs.insert(5, 0x8 << 20); // line in
        c.configs.insert(6, 0x2 << 20); // headphone out
        c
    }

    #[test]
    fn verbs() {
        assert_eq!(make_verb(0, 2, verb::SET_POWER_STATE, 0), 0x0027_0500);
        assert_eq!(make_verb(1, 3, verb::SET_FORMAT, 0x0011), 0x1032_0011);
        assert_eq!(make_verb(0, 0, verb::GET_PARAMETER, 4), 0x000F_0004);
    }

    #[test]
    fn output_path_through_a_mixer() {
        let mut c = codec();
        let path = find_output(&mut c).expect("a path");
        assert_eq!(
            path,
            Path {
                afg: 1,
                dac: 2,
                middle: 3,
                pin: 6,
                pin_input: 1
            }
        );
        prepare(&mut c, &path);
        assert!(c.sent.contains(&(2, verb::SET_STREAM_CHANNEL, 0x10)));
        assert!(c.sent.contains(&(2, verb::SET_FORMAT, 0x0011)));
        assert!(c.sent.contains(&(6, verb::SET_PIN_CONTROL, 0x40)));
        assert!(c.sent.contains(&(6, verb::SET_CONNECTION_SELECT, 1)));
        // The DAC's output amp unmuted at the 0 dB step of the group.
        assert!(c.sent.contains(&(2, verb::SET_AMP, 0xB027)));
        // The mixer's input amp.
        assert!(c.sent.contains(&(3, verb::SET_AMP, 0x7027)));
    }

    #[test]
    fn no_output_pin_means_no_path() {
        let mut c = codec();
        c.configs.insert(6, 1 << 30);
        assert_eq!(find_output(&mut c), None);
    }

    #[test]
    fn long_connection_lists() {
        let mut c = Fake::default();
        c.params.insert((9, param::CONNECTION_LENGTH), 0x80 | 3);
        c.lists.insert(9, vec![0x10, 0x20, 0x30]);
        // The fake answers in short form; check the long-form decoding
        // through a codec that packs two 16-bit entries per response.
        struct Long;
        impl Codec for Long {
            fn send(&mut self, _: u32, verb: u32, payload: u32) -> u32 {
                match (verb, payload) {
                    (verb::GET_PARAMETER, _) => 0x80 | 3,
                    (verb::GET_CONNECTION_LIST, 0) => 0x0222 << 16 | 0x0111,
                    (verb::GET_CONNECTION_LIST, 2) => 0x0333,
                    _ => 0,
                }
            }
        }
        assert_eq!(connections(&mut Long, 9), vec![0x111, 0x222, 0x333]);
        assert_eq!(connections(&mut c, 9).len(), 3);
    }

    #[test]
    fn descriptor_list_entry() {
        let e = bdl_entry(0x1234_0000, 32768, true);
        assert_eq!(&e[0..8], &0x1234_0000u64.to_le_bytes());
        assert_eq!(&e[8..12], &32768u32.to_le_bytes());
        assert_eq!(e[12], 1);
    }
}
