//! 文本编码：自动检测 / 解码 / 回写（R3-15，FR-TXT-013）。
//!
//! 口径（与 BC 一致的部分）：
//! - BOM 优先：UTF-8 / UTF-16LE / UTF-16BE 的 BOM 决定编码，且**不进入比较内容**；
//! - 无 BOM 时：纯 ASCII 与合法 UTF-8 判为 UTF-8，其余交给统计检测（chardetng）；
//! - 显式指定编码时不做检测，解码失败即报错（不静默替换字符，否则会掩盖选错编码）；
//! - 保存按会话编码 + 原 BOM 回写；目标编码无法表示的字符报错，不写 NCR 实体。
//!
//! 未覆盖：UTF-32、ISO-2022-JP 之外的 7bit 变体、EBCDIC 等（encoding_rs 不含）。

use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

use crate::{VfsError, UTF8_BOM};

const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];

/// 文件开头的字节序标记。保存时原样写回，否则文件的编码签名被静默改掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bom {
    #[default]
    None,
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Bom {
    pub fn detect(bytes: &[u8]) -> Bom {
        if bytes.starts_with(UTF8_BOM) {
            Bom::Utf8
        } else if bytes.starts_with(UTF16_LE_BOM) {
            Bom::Utf16Le
        } else if bytes.starts_with(UTF16_BE_BOM) {
            Bom::Utf16Be
        } else {
            Bom::None
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Bom::None => &[],
            Bom::Utf8 => UTF8_BOM,
            Bom::Utf16Le => UTF16_LE_BOM,
            Bom::Utf16Be => UTF16_BE_BOM,
        }
    }

    /// BOM 对应的编码（无 BOM 返回 None）
    pub fn encoding(self) -> Option<&'static Encoding> {
        match self {
            Bom::None => None,
            Bom::Utf8 => Some(UTF_8),
            Bom::Utf16Le => Some(UTF_16LE),
            Bom::Utf16Be => Some(UTF_16BE),
        }
    }
}

/// 解码结果：文本 + 实际生效的编码标签 + 原文件 BOM（保存时三者都要用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    pub text: String,
    pub encoding: &'static str,
    pub bom: Bom,
}

/// 编码标签解析。`None` / 空串 / `"auto"` 表示自动检测。
pub fn resolve(label: Option<&str>) -> Result<Option<&'static Encoding>, VfsError> {
    match label
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"))
    {
        None => Ok(None),
        Some(l) => Encoding::for_label(l.as_bytes())
            .map(Some)
            .ok_or_else(|| VfsError::Encoding(format!("未知编码标签: {l}"))),
    }
}

/// 自动检测：BOM → ASCII/UTF-8 → 统计检测。
pub fn detect(bytes: &[u8]) -> &'static Encoding {
    if let Some(enc) = Bom::detect(bytes).encoding() {
        return enc;
    }
    if std::str::from_utf8(bytes).is_ok() {
        return UTF_8;
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    detector.guess(None, chardetng::Utf8Detection::Allow)
}

/// 解码。`label` 为 None 时自动检测；显式编码下解码失败报 [`VfsError::Encoding`]。
pub fn decode(bytes: &[u8], label: Option<&str>) -> Result<DecodedText, VfsError> {
    let bom = Bom::detect(bytes);
    let requested = resolve(label)?;
    // BOM 与显式编码冲突时以显式编码为准（用户可能就是要按别的编码看字节），
    // 但 BOM 字节本身始终剥掉——它不是文本内容。
    let enc = requested
        .or_else(|| bom.encoding())
        .unwrap_or_else(|| detect(bytes));
    let body = &bytes[bom.bytes().len()..];
    let text = enc
        .decode_without_bom_handling_and_without_replacement(body)
        .ok_or_else(|| {
            VfsError::Encoding(format!(
                "字节不是合法的 {} 文本（可换编码或选自动检测）",
                enc.name()
            ))
        })?;
    Ok(DecodedText {
        text: text.into_owned(),
        encoding: enc.name(),
        bom,
    })
}

/// 按编码 + BOM 编码为落盘字节。
///
/// 两个必须特殊处理的点：
/// - encoding_rs 的 `encode` 对 UTF-16LE/BE 会输出 UTF-8（Encoding Standard 的
///   output encoding 恒为 UTF-8），所以 UTF-16 自己按码元序列化；
/// - 目标编码表示不了的字符，`encode` 会写成 HTML 数字实体（`&#20013;`）。
///   对比工具不能这样静默改内容，一律报错让用户改选编码。
pub fn encode(text: &str, label: &str, bom: Bom) -> Result<Vec<u8>, VfsError> {
    let enc = resolve(Some(label))?
        .ok_or_else(|| VfsError::Encoding("保存必须指定具体编码（不能是 auto）".into()))?;
    let mut out = Vec::with_capacity(text.len() + bom.bytes().len());
    out.extend_from_slice(bom.bytes());
    if enc == UTF_16LE {
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    } else if enc == UTF_16BE {
        out.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    } else {
        let (bytes, _, unmappable) = enc.encode(text);
        if unmappable {
            return Err(VfsError::Encoding(format!(
                "内容含 {} 无法表示的字符，未写盘（请改选编码）",
                enc.name()
            )));
        }
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三套码表下字节各不相同的样本，用来证明"真的按所选编码解码"。
    /// Big5 用繁体：简体"对"不在 Big5 码表内（用简体会让编码夹具本身失败）。
    const CN: &str = "中文对比\n第二行\n";
    const TW: &str = "中文對比\n第二行\n";
    const JP: &str = "日本語のテキスト\n二行目\n";

    fn enc_bytes(text: &str, label: &str) -> Vec<u8> {
        let e = Encoding::for_label(label.as_bytes()).unwrap();
        let (b, _, bad) = e.encode(text);
        assert!(!bad, "{label} 无法编码样本");
        b.into_owned()
    }

    #[test]
    fn bom_detection_and_bytes() {
        assert_eq!(Bom::detect(&[0xEF, 0xBB, 0xBF, b'a']), Bom::Utf8);
        assert_eq!(Bom::detect(&[0xFF, 0xFE, 0x61, 0x00]), Bom::Utf16Le);
        assert_eq!(Bom::detect(&[0xFE, 0xFF, 0x00, 0x61]), Bom::Utf16Be);
        assert_eq!(Bom::detect(b"plain"), Bom::None);
        assert_eq!(Bom::detect(&[]), Bom::None);
        assert_eq!(Bom::Utf8.bytes(), UTF8_BOM);
        assert_eq!(Bom::None.bytes(), &[] as &[u8]);
    }

    #[test]
    fn ascii_and_utf8_are_detected_as_utf8() {
        assert_eq!(detect(b"plain ascii\n"), UTF_8);
        assert_eq!(detect(CN.as_bytes()), UTF_8);
        assert_eq!(detect(&[]), UTF_8);
    }

    #[test]
    fn legacy_cjk_bytes_are_detected_and_decoded() {
        for (label, sample) in [("gb18030", CN), ("big5", TW), ("shift_jis", JP)] {
            let bytes = enc_bytes(sample, label);
            assert_ne!(bytes, sample.as_bytes(), "{label} 字节应与 UTF-8 不同");
            let got = decode(&bytes, None).unwrap();
            assert_eq!(got.text, sample, "{label} 自动检测解码失败");
            assert_eq!(got.bom, Bom::None);
        }
    }

    #[test]
    fn explicit_label_overrides_detection() {
        let bytes = enc_bytes(CN, "gb18030");
        let got = decode(&bytes, Some("gbk")).unwrap();
        assert_eq!(got.text, CN);
        // encoding_rs 的 GBK 是独立静态（解码用 GBK 表，编码走 gb18030），
        // 名字原样返回，回写时按同一标签解析即可字节一致
        assert_eq!(got.encoding, "GBK");
        // 选错编码必须报错，而不是替换字符后假装成功
        assert!(decode(&bytes, Some("utf-8")).is_err());
    }

    #[test]
    fn unknown_label_is_rejected() {
        assert!(decode(b"x", Some("no-such-encoding")).is_err());
        assert!(resolve(Some("")).unwrap().is_none());
        assert!(resolve(Some("AUTO")).unwrap().is_none());
        assert!(resolve(None).unwrap().is_none());
    }

    #[test]
    fn utf16_round_trip_both_endiannesses() {
        for (label, bom, first) in [
            ("utf-16le", Bom::Utf16Le, 0x2Du8),
            ("utf-16be", Bom::Utf16Be, 0x4Eu8),
        ] {
            let bytes = encode(CN, label, bom).unwrap();
            assert_eq!(&bytes[..2], bom.bytes());
            assert_eq!(bytes[2], first, "{label} 首字节序不对");
            let got = decode(&bytes, None).unwrap();
            assert_eq!(got.text, CN, "{label} BOM 自动识别失败");
            assert_eq!(got.bom, bom);
            // 显式指定同一编码也要能解（BOM 已剥离）
            assert_eq!(decode(&bytes, Some(label)).unwrap().text, CN);
        }
    }

    #[test]
    fn utf16_encode_is_not_silently_utf8() {
        // encoding_rs 的 encode() 对 UTF-16 会输出 UTF-8——这条守住自实现分支
        let bytes = encode("A", "utf-16le", Bom::None).unwrap();
        assert_eq!(bytes, vec![0x41, 0x00]);
        let bytes = encode("A", "utf-16be", Bom::None).unwrap();
        assert_eq!(bytes, vec![0x00, 0x41]);
    }

    #[test]
    fn utf8_bom_is_stripped_and_rewritten() {
        let mut bytes = UTF8_BOM.to_vec();
        bytes.extend_from_slice(CN.as_bytes());
        let got = decode(&bytes, None).unwrap();
        assert_eq!(got.text, CN);
        assert_eq!(got.bom, Bom::Utf8);
        assert_eq!(encode(&got.text, got.encoding, got.bom).unwrap(), bytes);
    }

    #[test]
    fn legacy_round_trip_preserves_bytes() {
        for (label, sample) in [("gb18030", CN), ("big5", TW), ("shift_jis", JP)] {
            let bytes = enc_bytes(sample, label);
            let got = decode(&bytes, Some(label)).unwrap();
            assert_eq!(
                encode(&got.text, got.encoding, got.bom).unwrap(),
                bytes,
                "{label} 回写字节不一致"
            );
        }
    }

    #[test]
    fn unrepresentable_char_is_an_error_not_an_entity() {
        // '中' 在 windows-1252 里没有码位：必须报错，不能写成 &#20013;
        let err = encode(CN, "windows-1252", Bom::None).unwrap_err();
        assert!(matches!(err, VfsError::Encoding(_)), "{err:?}");
        assert!(err.to_string().contains("windows-1252"));
    }

    #[test]
    fn save_rejects_auto_label() {
        assert!(encode("x", "auto", Bom::None).is_err());
    }

    #[test]
    fn windows_1252_is_detected_for_latin_bytes() {
        let bytes = enc_bytes("café\n", "windows-1252");
        let got = decode(&bytes, None).unwrap();
        assert_eq!(got.text, "café\n");
    }
}
