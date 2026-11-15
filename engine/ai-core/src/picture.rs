//! P14：图片比较引擎（FR-PIC 核心数据面；Pro 视图族底座）。
//!
//! 模型：解码两侧 PNG/JPEG 等（`image` crate，按 feature 启用格式）→
//! 像素级比较（RGBA 通道欧氏距离 ≤ tolerance 视为相同）→ 网格分块
//! （block 大小可配）聚合差异区 → [`PictureDiffResult`]（相似度 + 差异块）。
//!
//! 尺寸不同（FR-PIC-002）：以两侧最大宽高为画布，短侧缺失像素计为
//! 「差异像素」（差异区出现在右侧/下侧边缘）——BC 左上对齐语义子集；
//! 缩放对齐（resize 对齐）属后续批次。
//!
//! 视图消费：差异块列表即闪烁/并排视图的高亮框数据源；UI 渲染属
//! P14 视图批次（本模块保持 headless 可全量单测）。

/// 图片比较选项（FR-PIC 可配项；对齐 BC Picture Compare 设置子集）
#[derive(Debug, Clone)]
pub struct PictureDiffOptions {
    /// 每通道容差（0-255；0 = 逐像素精确比较）
    pub tolerance: u8,
    /// 差异聚合块边长（像素；默认 32）
    pub block: u32,
}

impl Default for PictureDiffOptions {
    fn default() -> Self {
        PictureDiffOptions {
            tolerance: 8,
            block: 32,
        }
    }
}

/// 差异聚合块（边界框 + 块内差异像素占比）
#[derive(Debug, Clone, PartialEq)]
pub struct PictureDiffBlock {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// 块内差异像素 / 块内总像素（0.0-1.0）
    pub diff_ratio: f32,
}

/// 图片比较结果（视图高亮框数据源）
#[derive(Debug, Clone, PartialEq)]
pub struct PictureDiffResult {
    /// 画布尺寸（两侧最大宽高）
    pub width: u32,
    pub height: u32,
    /// 两侧原始尺寸（不同时即 FR-PIC-002 尺寸差异的呈现依据）
    pub left_size: (u32, u32),
    pub right_size: (u32, u32),
    /// 差异像素总数（含越界缺失像素）
    pub diff_pixels: u64,
    /// 画布总像素
    pub total_pixels: u64,
    /// 相似度百分比（100.0 = 全同；保留 2 位小数）
    pub similarity_pct: f64,
    /// 差异聚合块（按扫描序；仅含 diff_ratio > 0 的块）
    pub blocks: Vec<PictureDiffBlock>,
}

/// 解码单侧图片字节流为 RGBA8 画布（错误统一 InvalidEncoding 语义由调用方映射）
pub fn decode_rgba(data: &[u8]) -> Result<image::RgbaImage, String> {
    let img = image::load_from_memory(data).map_err(|e| format!("图片解码失败: {e}"))?;
    Ok(img.to_rgba8())
}

/// 核心比较：两张 RGBA 画布（尺寸可不同）→ 差异结果
pub fn diff_rgba(
    left: &image::RgbaImage,
    right: &image::RgbaImage,
    opts: &PictureDiffOptions,
) -> PictureDiffResult {
    let width = left.width().max(right.width());
    let height = left.height().max(right.height());
    let tol = i32::from(opts.tolerance);
    let block = opts.block.max(1);

    let mut diff_pixels: u64 = 0;
    let total_pixels = u64::from(width) * u64::from(height);
    // 差异像素网格计数（按块聚合，避免全图细粒度块爆炸）
    let blocks_x = width.div_ceil(block);
    let mut block_counts: Vec<u32> = vec![0; (blocks_x * height.div_ceil(block)) as usize];

    for y in 0..height {
        for x in 0..width {
            let lp = left.get_pixel_checked(x, y);
            let rp = right.get_pixel_checked(x, y);
            let diff = match (lp, rp) {
                (Some(l), Some(r)) => {
                    let dr = i32::from(l[0]) - i32::from(r[0]);
                    let dg = i32::from(l[1]) - i32::from(r[1]);
                    let db = i32::from(l[2]) - i32::from(r[2]);
                    let da = i32::from(l[3]) - i32::from(r[3]);
                    dr.abs() > tol || dg.abs() > tol || db.abs() > tol || da.abs() > tol
                }
                // 单侧缺失（尺寸不同边缘）：计差异（FR-PIC-002 左上对齐）
                _ => true,
            };
            if diff {
                diff_pixels += 1;
                let bi = (y / block) * blocks_x + (x / block);
                block_counts[bi as usize] += 1;
            }
        }
    }

    let mut blocks = Vec::new();
    for (bi, &cnt) in block_counts.iter().enumerate() {
        if cnt == 0 {
            continue;
        }
        let bx = (bi as u32 % blocks_x) * block;
        let by = (bi as u32 / blocks_x) * block;
        let bw = block.min(width - bx);
        let bh = block.min(height - by);
        let ratio = cnt as f32 / (u64::from(bw) * u64::from(bh)).max(1) as f32;
        blocks.push(PictureDiffBlock {
            x: bx,
            y: by,
            w: bw,
            h: bh,
            diff_ratio: ratio.min(1.0),
        });
    }

    let similarity_pct = if total_pixels == 0 {
        100.0
    } else {
        ((total_pixels - diff_pixels) as f64 / total_pixels as f64 * 100.0 * 100.0).round() / 100.0
    };

    PictureDiffResult {
        width,
        height,
        left_size: (left.width(), left.height()),
        right_size: (right.width(), right.height()),
        diff_pixels,
        total_pixels,
        similarity_pct,
        blocks,
    }
}

/// 便捷：两侧字节流直接比较（解码 + diff）
pub fn diff_bytes(
    left: &[u8],
    right: &[u8],
    opts: &PictureDiffOptions,
) -> Result<PictureDiffResult, String> {
    let l = decode_rgba(left)?;
    let r = decode_rgba(right)?;
    Ok(diff_rgba(&l, &r, opts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(c))
    }

    fn png_bytes(img: &RgbaImage) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn identical_images_are_100pct_same() {
        let img = solid(64, 64, [10, 20, 30, 255]);
        let r = diff_rgba(&img, &img, &PictureDiffOptions::default());
        assert_eq!(r.diff_pixels, 0);
        assert_eq!(r.similarity_pct, 100.0);
        assert!(r.blocks.is_empty());
    }

    #[test]
    fn tolerance_absorbs_small_noise() {
        let mut right = solid(64, 64, [10, 20, 30, 255]);
        // 每像素 +5（容差 8 内）
        for p in right.pixels_mut() {
            p[0] = p[0].saturating_add(5);
        }
        let opts = PictureDiffOptions {
            tolerance: 8,
            ..Default::default()
        };
        let r = diff_rgba(&solid(64, 64, [10, 20, 30, 255]), &right, &opts);
        assert_eq!(r.diff_pixels, 0, "容差内噪声不算差异");
        // 容差 0 则全部差异
        let r0 = diff_rgba(
            &solid(64, 64, [10, 20, 30, 255]),
            &right,
            &PictureDiffOptions {
                tolerance: 0,
                ..Default::default()
            },
        );
        assert_eq!(r0.diff_pixels, 64 * 64);
    }

    #[test]
    fn diff_region_produces_block() {
        let left = solid(64, 64, [0, 0, 0, 255]);
        let mut right = solid(64, 64, [0, 0, 0, 255]);
        // 右上角 16x16 涂白（落在 block=32 的 (1,0) 块）
        for y in 0..16 {
            for x in 48..64 {
                right.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let r = diff_rgba(&left, &right, &PictureDiffOptions::default());
        assert_eq!(r.diff_pixels, 256);
        assert_eq!(r.blocks.len(), 1);
        let b = &r.blocks[0];
        assert_eq!((b.x, b.y, b.w, b.h), (32, 0, 32, 32));
        assert!((b.diff_ratio - 0.25).abs() < 0.01, "256/1024 = 0.25");
        assert_eq!(r.similarity_pct, 93.75); // 3840/4096 = 93.75
    }

    #[test]
    fn size_mismatch_counts_missing_as_diff() {
        let left = solid(64, 64, [0, 0, 0, 255]);
        let right = solid(64, 32, [0, 0, 0, 255]); // 右侧矮 32 行
        let r = diff_rgba(&left, &right, &PictureDiffOptions::default());
        assert_eq!(r.diff_pixels, 64 * 32, "缺失的 32 行全部计差异");
        assert_eq!(r.width, 64);
        assert_eq!(r.height, 64);
        assert_eq!(r.left_size, (64, 64));
        assert_eq!(r.right_size, (64, 32));
    }

    #[test]
    fn png_bytes_roundtrip_and_corrupt() {
        let img = solid(32, 32, [1, 2, 3, 255]);
        let bytes = png_bytes(&img);
        let r = diff_bytes(&bytes, &bytes, &PictureDiffOptions::default()).unwrap();
        assert_eq!(r.similarity_pct, 100.0);
        // 损坏字节流
        assert!(diff_bytes(b"not an image", &bytes, &PictureDiffOptions::default()).is_err());
    }

    /// 像素解码经 PNG roundtrip 的全链路（含 compression 语义）
    #[test]
    fn diff_bytes_real_png_pair() {
        let mut left = solid(48, 48, [100, 150, 200, 255]);
        for y in 20..28 {
            for x in 8..16 {
                left.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let mut right = left.clone();
        for y in 20..28 {
            for x in 8..16 {
                right.put_pixel(x, y, Rgba([0, 255, 0, 255])); // 同区域换色
            }
        }
        let r = diff_bytes(
            &png_bytes(&left),
            &png_bytes(&right),
            &PictureDiffOptions::default(),
        )
        .unwrap();
        assert_eq!(r.diff_pixels, 64);
        assert!(!r.blocks.is_empty());
    }
}
