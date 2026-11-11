# 0011. 0011 Encode with libwebp (lossy WebP, quality 50 by default), with JPEG as an option

## Context
The target is 50-100 MB per month at defaults, which is about 25-50 KB per capture including the thumbnail. Decoding must be fast in the viewer. The `image` crate encodes WebP only losslessly.

## Options
- **PNG:** far too big.
- **JPEG** (`jpeg-encoder` to encode, `zune-jpeg` to decode): fast, but text edges look worse at small sizes.
- **Lossy WebP** through the `webp` crate (libwebp, built from bundled source): about 25-35 % smaller than JPEG for screen content, and libwebp decodes fast.
- **AVIF:** smaller, but encoding is slow and costs CPU in the agent.

## Decision
- Default: WebP at quality 50, 1280 px wide.
- Thumbnail: WebP at quality 40, 240 px wide.
- JPEG is available as a setting.
- Scaling uses `fast_image_resize` (SIMD), and the agent never upscales.

## Consequences
libwebp is compiled through `cc`, with no extra runtime package. A fixture test in CI checks the average sizes. If real use exceeds the target, the default quality drops to 40.
