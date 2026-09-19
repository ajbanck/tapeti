//! Headless screenshots: what the app would draw, as a PNG, with no window.
//!
//! `npm run smoke` drives the browser build through headless Chrome and leaves
//! screenshots in `scratch/`. The desktop build had no equivalent — `app.rs`
//! draws real frames in its tests, but nothing looks at the pixels, so "the row
//! is not rendering properly" could only be answered by someone with the app on
//! screen. This closes that: egui hands over tessellated triangles and its font
//! atlas, and neither needs a GPU to become an image.
//!
//! The rasteriser is the small half of what `egui_glow` does: one texture, no
//! shaders, premultiplied `Color32` throughout (epaint's convention), and the
//! blend egui asks for, `dst = src + dst·(1 − src.a)`. Colours are multiplied in
//! gamma space rather than linear, which the real painter does not do, so a
//! screenshot is a faithful picture of *layout and text* and approximate about
//! the last few values of a blend.

use std::collections::HashMap;

use egui::epaint::{ClippedPrimitive, Primitive, Vertex};
use egui::{Color32, Rect, TextureId};

/// An RGBA image being painted into, premultiplied like everything in epaint.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pixels: Vec<Color32>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![Color32::BLACK; width * height] }
    }

    /// Start from the colour eframe clears the window to, so a pixel nothing
    /// covers is the app's background and not black.
    fn clear(&mut self, colour: Color32) {
        self.pixels.fill(colour);
    }

    fn blend(&mut self, x: usize, y: usize, src: Color32) {
        if src.a() == 0 {
            return;
        }
        let dst = self.pixels[y * self.width + x];
        let inv = 255 - src.a() as u32;
        let mix = |s: u8, d: u8| (s as u32 + (d as u32 * inv) / 255).min(255) as u8;
        self.pixels[y * self.width + x] = Color32::from_rgba_premultiplied(
            mix(src.r(), dst.r()),
            mix(src.g(), dst.g()),
            mix(src.b(), dst.b()),
            mix(src.a(), dst.a()),
        );
    }

    /// How many pixels differ from another canvas of the same size: what a test
    /// asks when the question is whether something was drawn at all.
    #[cfg(test)]
    pub fn diff(&self, other: &Canvas) -> usize {
        self.pixels.iter().zip(&other.pixels).filter(|(a, b)| a != b).count()
    }

    /// PNG bytes, the same encoder the screen view saves with.
    pub fn to_png(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            rgba.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
        }
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&rgba).expect("png data");
        }
        out
    }
}

/// The textures egui has handed out so far, kept between frames the way a real
/// painter keeps them: the font atlas arrives once and is then patched.
#[derive(Default)]
pub struct Textures(HashMap<TextureId, egui::ColorImage>);

impl Textures {
    /// Takes the deltas, the way a real painter does: epaint asserts on dropping
    /// a delta nobody applied.
    pub fn apply(&mut self, delta: &mut egui::epaint::textures::TexturesDelta) {
        for (id, patches) in &delta.set {
            for delta in patches {
                let egui::epaint::image::ImageData::Color(patch) = &delta.image;
                match delta.pos {
                    // A whole texture.
                    None => {
                        self.0.insert(*id, (**patch).clone());
                    }
                    // A patch of one: the atlas grows a glyph at a time.
                    Some([x0, y0]) => {
                        if let Some(image) = self.0.get_mut(id) {
                            for y in 0..patch.height() {
                                for x in 0..patch.width() {
                                    let at = (y0 + y) * image.width() + x0 + x;
                                    if at < image.pixels.len() {
                                        image.pixels[at] = patch.pixels[y * patch.width() + x];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.0.remove(id);
        }
        delta.clear();
    }

    /// Nearest-neighbour sample; egui's own atlas is drawn 1:1 at this scale.
    fn sample(&self, id: TextureId, u: f32, v: f32) -> Color32 {
        let Some(image) = self.0.get(&id) else { return Color32::WHITE };
        let x = ((u * image.width() as f32) as isize).clamp(0, image.width() as isize - 1) as usize;
        let y = ((v * image.height() as f32) as isize).clamp(0, image.height() as isize - 1) as usize;
        image.pixels[y * image.width() + x]
    }
}

/// Paint what egui tessellated into `canvas`.
pub fn paint(canvas: &mut Canvas, textures: &Textures, primitives: &[ClippedPrimitive]) {
    for ClippedPrimitive { clip_rect, primitive } in primitives {
        let Primitive::Mesh(mesh) = primitive else { continue };
        for t in 0..mesh.indices.len() / 3 {
            let i = &mesh.indices[t * 3..t * 3 + 3];
            let v =
                [&mesh.vertices[i[0] as usize], &mesh.vertices[i[1] as usize], &mesh.vertices[i[2] as usize]];
            fill(canvas, textures, mesh.texture_id, v, *clip_rect);
        }
    }
}

/// One triangle, barycentric, clipped to the primitive's scissor rectangle.
fn fill(canvas: &mut Canvas, textures: &Textures, id: TextureId, v: [&Vertex; 3], clip: Rect) {
    let (x0, x1) = (v[0].pos.x.min(v[1].pos.x).min(v[2].pos.x), v[0].pos.x.max(v[1].pos.x).max(v[2].pos.x));
    let (y0, y1) = (v[0].pos.y.min(v[1].pos.y).min(v[2].pos.y), v[0].pos.y.max(v[1].pos.y).max(v[2].pos.y));
    let left = x0.max(clip.left()).max(0.0).floor() as usize;
    let right = (x1.min(clip.right()).min(canvas.width as f32).ceil() as usize).min(canvas.width);
    let top = y0.max(clip.top()).max(0.0).floor() as usize;
    let bottom = (y1.min(clip.bottom()).min(canvas.height as f32).ceil() as usize).min(canvas.height);

    let area = edge(v[0], v[1], v[2].pos.x, v[2].pos.y);
    if area.abs() < 1e-6 {
        return;
    }
    for y in top..bottom {
        for x in left..right {
            // Pixel centres, so a rectangle of two triangles has no seam.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(v[1], v[2], px, py) / area;
            let w1 = edge(v[2], v[0], px, py) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
            let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
            let texel = textures.sample(id, u, t);
            let at = |f: fn(&Color32) -> u8| {
                w0 * f(&v[0].color) as f32 + w1 * f(&v[1].color) as f32 + w2 * f(&v[2].color) as f32
            };
            let mul = |c: f32, t: u8| ((c * t as f32) / 255.0).round().clamp(0.0, 255.0) as u8;
            canvas.blend(
                x,
                y,
                Color32::from_rgba_premultiplied(
                    mul(at(|c| c.r()), texel.r()),
                    mul(at(|c| c.g()), texel.g()),
                    mul(at(|c| c.b()), texel.b()),
                    mul(at(|c| c.a()), texel.a()),
                ),
            );
        }
    }
}

fn edge(a: &Vertex, b: &Vertex, px: f32, py: f32) -> f32 {
    (px - a.pos.x) * (b.pos.y - a.pos.y) - (py - a.pos.y) * (b.pos.x - a.pos.x)
}

/// Draw `frames` frames of `app` at this size and return the last one as a PNG.
/// More than one frame because egui lays out on the size it measured last time:
/// a modal is not where it will be until the frame after it opens.
pub fn capture(ctx: &egui::Context, app: &mut crate::app::App, size: (f32, f32), frames: usize) -> Canvas {
    capture_with(ctx, app, size, frames, |_| Vec::new())
}

/// The same, with the events of each frame decided by the caller: a test that
/// wants a picture of something it has to click open first.
pub fn capture_with(
    ctx: &egui::Context,
    app: &mut crate::app::App,
    size: (f32, f32),
    frames: usize,
    mut events: impl FnMut(usize) -> Vec<egui::Event>,
) -> Canvas {
    let mut textures = Textures::default();
    let mut canvas = Canvas::new(size.0 as usize, size.1 as usize);
    for frame in 0..frames.max(1) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))),
            events: events(frame),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.frame(ui));
        textures.apply(&mut output.textures_delta);
        if frame + 1 == frames.max(1) {
            canvas.clear(ctx.global_style().visuals.panel_fill);
            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            paint(&mut canvas, &textures, &primitives);
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;
    use crate::settings::Settings;
    use crate::state::Store;
    use tapeti_core::types::{create_body, Block, CREATABLE_IDS};

    /// A screen block whose attributes make every pixel red, so the thumbnail is
    /// a colour that appears nowhere else in the app.
    fn red_screen() -> Block {
        let mut data = vec![0xffu8];
        data.extend(std::iter::repeat_n(0x00, 6144)); // bitmap: all paper
        data.extend(std::iter::repeat_n(0x12, 768)); // ink 2, paper 2: red on red
        let checksum = data.iter().fold(0u8, |a, b| a ^ b);
        data.push(checksum);
        Block::new(tapeti_core::types::Body::Standard { pause: 1000, data })
    }

    fn red_pixels(canvas: &Canvas) -> usize {
        canvas.pixels.iter().filter(|p| p.r() > 140 && p.g() < 70 && p.b() < 70).count()
    }

    /// The editor lays its thumbnail beside the fields, and egui clips what does
    /// not fit without so much as a scrollbar — so in a narrow pane the preview
    /// used to be half an image against the pane edge, or nothing at all. The row
    /// wraps now, and this is the check that it still does at a width nobody
    /// develops at.
    #[test]
    fn the_screen_preview_survives_a_narrow_pane() {
        for width in [1400.0, 1000.0, 820.0] {
            let ctx = egui::Context::default();
            let mut store = Store::new(Settings::default());
            store.tape_mut(0).load("demo.tap".into(), None, vec![red_screen()], None);
            store.set_cursor(0, 0, crate::state::SelectMode::Single);
            let mut app =
                crate::app::App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
            let canvas = capture(&ctx, &mut app, (width, 620.0), 3);
            let red = red_pixels(&canvas);
            assert!(red > 2000, "at {width} points wide the screen preview drew {red} pixels");
        }
    }

    /// A screenshot with something in it: not a blank canvas, and not one flat
    /// colour either — which is what a broken font atlas or an empty mesh list
    /// would produce, and what nobody would notice in a test that only draws.
    #[test]
    fn draws_pixels_of_more_than_one_colour() {
        let ctx = egui::Context::default();
        let mut store = Store::new(Settings::default());
        let blocks: Vec<Block> = CREATABLE_IDS.iter().map(|id| Block::new(create_body(*id))).collect();
        store.tape_mut(0).load("demo.tzx".into(), None, blocks, None);
        let mut app =
            crate::app::App::build(&ctx, Menu::headless(), store, std::time::Instant::now(), 0, false);
        let canvas = capture(&ctx, &mut app, (900.0, 600.0), 2);

        let png = canvas.to_png();
        assert!(png.len() > 1000, "a PNG of a whole window should not be tiny");
        let mut seen: Vec<Color32> = Vec::new();
        for p in &canvas.pixels {
            if !seen.contains(p) {
                seen.push(*p);
            }
            if seen.len() > 20 {
                break;
            }
        }
        assert!(seen.len() > 20, "the frame came out in {} colours; text or shapes are missing", seen.len());
    }
}
