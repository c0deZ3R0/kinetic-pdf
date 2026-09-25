//! Drawing shapes with OpenGL. Every shape -- a piece of line or a filled
//! triangle -- is an instance of the same six vertices, so a page of a million
//! shapes is one buffer upload, then a draw call per run of shapes sharing a
//! blend, painted in the page's own order. A line's six vertices make a quad
//! stretched between its ends and widened in the vertex shader; a triangle
//! uses three of them for its corners and folds the other three away. A line
//! with round ends is a capsule: its quad reaches past the ends and the
//! fragment shader measures the distance to the segment. An image's quad is
//! its parallelogram, sampling its place in the atlas, whose pages are one
//! texture array, so images need no change of texture either.
//!
//! Clips come two ways. A convex clip set is a list of half-planes, kept in a
//! float texture: each shape carries its set, and the fragment shader fades
//! its coverage across every edge, so clipped shapes still draw together. Any
//! other clip set splits the run, and is drawn into the stencil buffer first
//! -- each of its shapes counting up where the ones before it all covered --
//! so the run shows only where every shape overlaps.

use std::ops::Range;

use glow::HasContext;

use crate::atlas::ATLAS_SIZE;
use crate::geometry::Matrix;
use crate::{Blend, Primitive, Run, Shapes, Style};

/// Maps page space to the viewport, for both programs.
const TRANSFORM: &str = r#"
uniform mat3 u_page_to_pixels;
uniform vec2 u_screen;

vec2 to_pixels(vec2 p) {
    return (u_page_to_pixels * vec3(p, 1.0)).xy;
}

vec4 at_pixel(vec2 pixel) {
    return vec4(pixel.x / u_screen.x * 2.0 - 1.0, 1.0 - pixel.y / u_screen.y * 2.0, 0.0, 1.0);
}
"#;

/// Reads the clip texture, in both of the shapes' shaders. Texels hold, first,
/// each clip set's first half-plane and how many; then the half-planes.
const PLANES: &str = r#"
uniform sampler2D u_planes;
const int PLANES_WIDTH = 1024;

vec4 plane_texel(int index) {
    return texelFetch(u_planes, ivec2(index % PLANES_WIDTH, index / PLANES_WIDTH), 0);
}
"#;

/// Texels to a row of the clip texture; `PLANES_WIDTH` in the shaders.
const PLANES_WIDTH: usize = 1024;

/// Reads what a shape is drawn with from the styles texture: two texels each,
/// the width, kind and clip, then the colour. Hundreds of thousands of shapes
/// share a few thousand styles, so each shape carries only an index.
const STYLES: &str = r#"
uniform sampler2D u_styles;
const int STYLES_WIDTH = 1024;

vec4 style_texel(int index) {
    return texelFetch(u_styles, ivec2(index % STYLES_WIDTH, index / STYLES_WIDTH), 0);
}
"#;

/// Texels to a row of the styles texture; `STYLES_WIDTH` in the shader.
const STYLES_WIDTH: usize = 1024;

const SHAPE_VERTEX: &str = r#"
layout(location = 0) in vec3 a_corner;
layout(location = 1) in vec2 a_p0;
layout(location = 2) in vec2 a_p1;
layout(location = 3) in vec2 a_p2;
layout(location = 4) in uint a_style;

uniform float u_pixels_per_point;
uniform mat3 u_pixels_to_page;
// Places in the atlas are fractions of a full page; its texture holds only the
// rows in use, so they're stretched to fit.
uniform vec2 u_atlas_scale;

out vec4 v_colour;
out float v_across;
out float v_along;
out float v_length;
out float v_half;
out vec2 v_page;
flat out int v_clip_start;
flat out int v_clip_count;
out vec2 v_uv;
flat out int v_atlas_page;

void main() {
    vec4 style = style_texel(int(a_style) * 2);
    float a_width = style.x;
    float a_clip = style.z;
    vec4 a_colour = style_texel(int(a_style) * 2 + 1);

    vec2 position;
    int kind = int(style.y + 0.5);
    v_along = 0.0;
    v_length = 0.0;
    v_uv = vec2(0.0);
    v_atlas_page = -1;
    if (kind == 1) {
        // A triangle: corners 0, 1 and 2, the rest folded onto corner 0.
        int corner = int(a_corner.z + 0.5);
        position = to_pixels(corner == 1 ? a_p1 : corner == 2 ? a_p2 : a_p0);
        v_across = 0.0;
        v_half = 1.0e6;
        v_colour = a_colour;
    } else if (kind >= 3) {
        // An image: x runs 0 to 1 along its bottom, y -1 to 1 up its side;
        // its top row is at the top of its place in the atlas.
        float right = a_corner.x;
        float up = (a_corner.y + 1.0) * 0.5;
        position = to_pixels(a_p0 + (a_p1 - a_p0) * right + (a_p2 - a_p0) * up);
        v_uv = vec2(mix(a_colour.x, a_colour.z, right), mix(a_colour.w, a_colour.y, up)) * u_atlas_scale;
        v_atlas_page = kind - 3;
        v_across = 0.0;
        v_half = 1.0e6;
        v_colour = vec4(1.0, 1.0, 1.0, a_width);
    } else {
        vec2 p0 = to_pixels(a_p0);
        vec2 p1 = to_pixels(a_p1);
        vec2 d = p1 - p0;
        float len = length(d);
        vec2 along = len > 1e-4 ? d / len : vec2(1.0, 0.0);
        vec2 across = vec2(-along.y, along.x);

        // Hairlines, and lines thinner than a pixel, are drawn a pixel wide
        // at full strength, as pdfium draws them: dense hatching of very thin
        // lines then fills in solid, as it does there.
        float width = max(a_width * u_pixels_per_point, 1.0);

        // A pixel of fringe each side for anti-aliasing. Round ends reach
        // half the width past each end, measured from the segment in the
        // fragment shader; square ones half a pixel, so the pieces of a
        // flattened curve meet.
        float half_width = width * 0.5 + 1.0;
        bool round_ends = kind == 2;
        float reach = round_ends ? half_width : 0.5;
        float from_start = a_corner.x * (len + 2.0 * reach) - reach;
        position = p0 + along * from_start + across * (a_corner.y * half_width);
        v_across = a_corner.y * half_width;
        if (round_ends) {
            v_along = from_start;
            v_length = len;
        }
        v_half = width * 0.5;
        v_colour = a_colour;
    }
    if (a_clip > 0.5) {
        vec4 set = plane_texel(int(a_clip + 0.5) - 1);
        v_clip_start = int(set.x + 0.5);
        v_clip_count = int(set.y + 0.5);
    } else {
        v_clip_start = 0;
        v_clip_count = 0;
    }
    v_page = (u_pixels_to_page * vec3(position, 1.0)).xy;
    gl_Position = at_pixel(position);
}
"#;

const SHAPE_FRAGMENT: &str = r#"
uniform float u_pixels_per_point;
uniform sampler2DArray u_atlas;

in vec4 v_colour;
in float v_across;
in float v_along;
in float v_length;
in float v_half;
in vec2 v_page;
flat in int v_clip_start;
flat in int v_clip_count;
in vec2 v_uv;
flat in int v_atlas_page;
out vec4 frag_colour;

void main() {
    // Distance from the line, past its ends only for round ones (whose
    // v_along runs past 0 and v_length); 0 across a triangle.
    float beyond = max(max(-v_along, v_along - v_length), 0.0);
    float coverage = clamp(v_half + 0.5 - length(vec2(beyond, v_across)), 0.0, 1.0);
    // Inside every half-plane of the clip, fading over a pixel at each edge.
    for (int i = 0; i < v_clip_count; i++) {
        vec3 plane = plane_texel(v_clip_start + i).xyz;
        coverage *= clamp(dot(plane, vec3(v_page, 1.0)) * u_pixels_per_point + 0.5, 0.0, 1.0);
    }
    if (coverage <= 0.0) {
        discard;
    }
    // Sampled for every shape, so filtering has its derivatives everywhere;
    // the atlas holds premultiplied colours.
    vec4 texel = texture(u_atlas, vec3(v_uv, float(max(v_atlas_page, 0))));
    vec4 colour = v_atlas_page >= 0 ? texel * v_colour.a : vec4(v_colour.rgb * v_colour.a, v_colour.a);
    frag_colour = colour * coverage;
}
"#;

const CLIP_VERTEX: &str = r#"
layout(location = 0) in vec2 a_position;

void main() {
    gl_Position = at_pixel(to_pixels(a_position));
}
"#;

const CLIP_FRAGMENT: &str = r#"
out vec4 frag_colour;

void main() {
    frag_colour = vec4(0.0);
}
"#;

/// A texture laid over a box of the view, corners from the vertex number
/// alone, so it needs no buffer.
const BLIT_VERTEX: &str = r#"
uniform vec4 u_box;
uniform vec2 u_screen;
out vec2 v_uv;

void main() {
    vec2 corners[6] = vec2[6](vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    vec2 corner = corners[gl_VertexID];
    vec2 pixel = u_box.xy + corner * u_box.zw;
    gl_Position = vec4(pixel.x / u_screen.x * 2.0 - 1.0, 1.0 - pixel.y / u_screen.y * 2.0, 0.0, 1.0);
    // A canvas's rows run from the bottom, as OpenGL draws them.
    v_uv = vec2(corner.x, 1.0 - corner.y);
}
"#;

const BLIT_FRAGMENT: &str = r#"
uniform sampler2D u_texture;
in vec2 v_uv;
out vec4 frag_colour;

void main() {
    frag_colour = texture(u_texture, v_uv);
}
"#;

/// Where each of a `Primitive`'s points sits: attribute index, number of
/// floats, byte offset. Its style follows them, as a whole number
/// (`STYLE_ATTRIBUTE`).
const ATTRIBUTES: [(u32, i32, i32); 3] = [(1, 2, 0), (2, 2, 8), (3, 2, 16)];

/// The style attribute: index, and its byte offset in a `Primitive`.
const STYLE_ATTRIBUTE: (u32, i32) = (4, 24);

/// Clip shapes a stencilled run can be within, at most: the stencil counts to
/// 255.
const DEEPEST_CLIP: usize = 255;

/// The shaders' first lines for this context: GLSL 3.30 on desktop OpenGL,
/// 3.00 ES on OpenGL ES and WebGL 2. Instanced drawing needs one of those.
fn header(gl: &glow::Context) -> &'static str {
    let version = unsafe { gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION) };
    if version.contains("ES") {
        "#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2DArray;\n"
    } else {
        "#version 330 core\n"
    }
}

/// A linked program from a vertex and a fragment shader's sources, each made
/// of the pieces given.
unsafe fn program(gl: &glow::Context, vertex: &[&str], fragment: &[&str]) -> Result<glow::Program, String> {
    let program = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, pieces) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, &[&[header(gl)], pieces].concat().concat());
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            return Err(format!("shader didn't compile: {}", gl.get_shader_info_log(shader)));
        }
        gl.attach_shader(program, shader);
        shaders.push(shader);
    }
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        return Err(format!("shaders didn't link: {}", gl.get_program_info_log(program)));
    }
    for shader in shaders {
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
    }
    Ok(program)
}

/// Words in the OpenGL renderer's name that mean it draws in software, with
/// no GPU driver: Mesa's llvmpipe and softpipe, Google's SwiftShader, and
/// Windows' fallbacks, as in some virtual machines and remote desktops.
const SOFTWARE_RENDERERS: [&str; 5] = ["llvmpipe", "softpipe", "swiftshader", "microsoft basic render", "gdi generic"];

/// Whether an OpenGL renderer of this name draws in software.
fn is_software_renderer(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SOFTWARE_RENDERERS.iter().any(|software| name.contains(software))
}

/// A texture to bind at `target`, sampled with `filter`, clamped at its edges.
unsafe fn texture(gl: &glow::Context, target: u32, filter: u32) -> Result<glow::Texture, String> {
    let texture = gl.create_texture()?;
    gl.bind_texture(target, Some(texture));
    for (parameter, value) in [
        (glow::TEXTURE_MIN_FILTER, filter),
        (glow::TEXTURE_MAG_FILTER, filter),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(target, parameter, value as i32);
    }
    gl.bind_texture(target, None);
    Ok(texture)
}

/// A column-major 3 x 3 matrix for GLSL, from a PDF matrix.
fn mat3(Matrix([a, b, c, d, e, f]): Matrix) -> [f32; 9] {
    [a, b, 0.0, c, d, 0.0, e, f, 1.0]
}

/// A program's page-to-viewport uniforms.
struct Transform {
    page_to_pixels: Option<glow::UniformLocation>,
    screen: Option<glow::UniformLocation>,
}

impl Transform {
    unsafe fn of(gl: &glow::Context, program: glow::Program) -> Self {
        Transform { page_to_pixels: gl.get_uniform_location(program, "u_page_to_pixels"), screen: gl.get_uniform_location(program, "u_screen") }
    }

    /// Sets the uniforms of the program in use.
    unsafe fn set(&self, gl: &glow::Context, page_to_pixels: Matrix, screen: [f32; 2]) {
        gl.uniform_matrix_3_f32_slice(self.page_to_pixels.as_ref(), false, &mat3(page_to_pixels));
        gl.uniform_2_f32(self.screen.as_ref(), screen[0], screen[1]);
    }
}

/// The clip texture's texels, as RGBA floats: each set's first half-plane
/// and count (none for a set drawn through the stencil), then the half-planes;
/// padded to whole rows.
fn plane_texels(shapes: &Shapes) -> Vec<f32> {
    let sets = &shapes.clips.planes;
    let mut texels: Vec<f32> = Vec::with_capacity((sets.len() + sets.iter().flatten().map(Vec::len).sum::<usize>()) * 4);
    let mut next = sets.len();
    for planes in sets {
        let count = planes.as_ref().map_or(0, Vec::len);
        texels.extend([next as f32, count as f32, 0.0, 0.0]);
        next += count;
    }
    for [a, b, c] in sets.iter().flatten().flatten() {
        texels.extend([*a, *b, *c, 0.0]);
    }
    let rows = (texels.len() / 4).div_ceil(PLANES_WIDTH).max(1);
    texels.resize(rows * PLANES_WIDTH * 4, 0.0);
    texels
}

/// The shaders and vertex layouts that draw any page's uploaded shapes,
/// made once for a context.
pub struct Renderer {
    shape_program: glow::Program,
    shape_transform: Transform,
    pixels_per_point: Option<glow::UniformLocation>,
    pixels_to_page: Option<glow::UniformLocation>,
    planes_sampler: Option<glow::UniformLocation>,
    styles_sampler: Option<glow::UniformLocation>,
    atlas_sampler: Option<glow::UniformLocation>,
    atlas_scale: Option<glow::UniformLocation>,
    shape_vertex_array: glow::VertexArray,
    corners: glow::Buffer,
    clip_program: glow::Program,
    clip_transform: Transform,
    clip_vertex_array: glow::VertexArray,
    /// The marks painted with a page, sent again each time, with the styles
    /// that draw them.
    marks: glow::Buffer,
    mark_styles: glow::Texture,
    /// Lays a canvas's texture over the view (`blit`).
    blit_program: glow::Program,
    blit_box: Option<glow::UniformLocation>,
    blit_screen: Option<glow::UniformLocation>,
    blit_sampler: Option<glow::UniformLocation>,
    blit_vertex_array: glow::VertexArray,
}

/// The styles texture's texels: two to a style, the width, kind and clip,
/// then the colour; padded to whole rows.
fn style_texels(styles: &[Style]) -> Vec<f32> {
    let mut texels: Vec<f32> = Vec::with_capacity(styles.len() * 8);
    for style in styles {
        texels.extend([style.width, style.kind, style.clip, 0.0]);
        texels.extend(style.colour);
    }
    let rows = (texels.len() / 4).div_ceil(STYLES_WIDTH).max(1);
    texels.resize(rows * STYLES_WIDTH * 4, 0.0);
    texels
}

/// Puts `styles` in `texture`, as the shapes' shader reads them.
unsafe fn upload_styles(gl: &glow::Context, texture: glow::Texture, styles: &[Style]) {
    let texels = style_texels(styles);
    let rows = (texels.len() / 4 / STYLES_WIDTH) as i32;
    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    let pixels = glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&texels)));
    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, STYLES_WIDTH as i32, rows, 0, glow::RGBA, glow::FLOAT, pixels);
    gl.bind_texture(glow::TEXTURE_2D, None);
}

/// A highlighter's mark over a page: a rectangle -- left, bottom, right, top
/// in page points -- whose colour multiplies what's under it, so paper takes
/// the colour and lines stay dark, however the page is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub rect: [f32; 4],
    pub colour: [f32; 3],
}

/// Sets how shapes painted with `blend` combine with what's drawn: colours
/// are premultiplied by alpha.
unsafe fn set_blend(gl: &glow::Context, blend: Blend) {
    match blend {
        Blend::Normal => gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE),
        // Over an opaque page: source times what's there, plus what's there
        // where the source is see-through.
        Blend::Multiply => gl.blend_func_separate(glow::DST_COLOR, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE),
    }
}

/// Pixels added around a run's bounds before it's culled or scissored: a
/// hairline is drawn a pixel wide at any zoom, and edges are anti-aliased
/// past where they fall.
const CULL_MARGIN: f32 = 2.0;

/// The bounds of `shapes`' primitives in `range`, in page points: left,
/// bottom, right, top. A line reaches half its width past its ends and its
/// sides; an image fills out the parallelogram its three corners make.
fn primitive_bounds(shapes: &Shapes, range: Range<usize>) -> [f32; 4] {
    let mut bounds = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    let mut take = |[x, y]: [f32; 2], pad: f32| {
        bounds = [bounds[0].min(x - pad), bounds[1].min(y - pad), bounds[2].max(x + pad), bounds[3].max(y + pad)];
    };
    for primitive in shapes.primitives.get(range).unwrap_or_default() {
        let style = shapes.style_of(primitive);
        let [a, b, c] = primitive.points;
        if style.is_image() {
            for corner in [a, b, c, [b[0] + c[0] - a[0], b[1] + c[1] - a[1]]] {
                take(corner, 0.0);
            }
        } else if style.is_triangle() {
            for corner in [a, b, c] {
                take(corner, 0.0);
            }
        } else {
            let pad = style.width.abs() / 2.0;
            take(a, pad);
            take(b, pad);
        }
    }
    bounds
}

/// The overlap of the bounds of clip set `set`'s shapes, in page points.
fn set_bounds(shapes: &Shapes, set: &[usize]) -> [f32; 4] {
    let clips = &shapes.clips;
    set.iter().fold([f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY], |overlap, &shape| {
        let vertices = clips.shapes.get(shape).and_then(|range| clips.vertices.get(range.clone())).unwrap_or_default();
        let [left, bottom, right, top] = vertices.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, &[x, y]| {
            [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
        });
        [overlap[0].max(left), overlap[1].max(bottom), overlap[2].min(right), overlap[3].min(top)]
    })
}

/// Page-point bounds as a box of window pixels -- x, y from the bottom left,
/// width and height -- for a view drawn by `page_to_pixels` into `viewport`,
/// `screen` pixels in size with its origin at the top left, widened by
/// `CULL_MARGIN`. Empty bounds give an empty box.
fn window_box([left, bottom, right, top]: [f32; 4], page_to_pixels: Matrix, viewport: [i32; 4], screen: [f32; 2]) -> [i32; 4] {
    if !(left <= right && bottom <= top) {
        return [0, 0, 0, 0];
    }
    let (mut low, mut high) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for corner in [[left, bottom], [right, bottom], [left, top], [right, top]] {
        let [x, y] = page_to_pixels.apply(corner);
        low = [low[0].min(x), low[1].min(y)];
        high = [high[0].max(x), high[1].max(y)];
    }
    let [sx, sy] = [viewport[2] as f32 / screen[0].max(1.0), viewport[3] as f32 / screen[1].max(1.0)];
    let x0 = viewport[0] as f32 + low[0] * sx - CULL_MARGIN;
    let x1 = viewport[0] as f32 + high[0] * sx + CULL_MARGIN;
    // Rows count down from the viewport's top.
    let y0 = viewport[1] as f32 + viewport[3] as f32 - high[1] * sy - CULL_MARGIN;
    let y1 = viewport[1] as f32 + viewport[3] as f32 - low[1] * sy + CULL_MARGIN;
    let clamp = |v: f32| v.clamp(-1e9, 1e9).floor() as i64;
    let (x0, y0, x1, y1) = (clamp(x0), clamp(y0), clamp(x1.ceil()), clamp(y1.ceil()));
    let fit = |v: i64| v.clamp(i32::MIN as i64 / 2, i32::MAX as i64 / 2) as i32;
    [fit(x0), fit(y0), fit(x1 - x0), fit(y1 - y0)]
}

/// The overlap of two boxes of window pixels, or `None` if they don't.
fn intersect(a: [i32; 4], b: [i32; 4]) -> Option<[i32; 4]> {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    (x1 > x0 && y1 > y0).then(|| [x0, y0, x1 - x0, y1 - y0])
}

/// A page's shapes made ready for the GPU, on any thread: laid out as the
/// shaders read them, with the bounds culling goes by. Worked out as an upload
/// began, this held up that frame by as much as 40 ms on a heavy drawing
/// sheet; made where the page is read, the frame only sends it.
pub struct Prepared {
    shapes: Shapes,
    planes: Vec<f32>,
    styles: Vec<f32>,
    runs: Vec<Run>,
    run_bounds: Vec<[f32; 4]>,
    set_bounds: Vec<[f32; 4]>,
}

impl Prepared {
    pub fn new(shapes: Shapes) -> Prepared {
        let runs = if shapes.runs.is_empty() {
            vec![Run { start: 0, len: shapes.primitives.len(), blend: Blend::Normal, clip: None }]
        } else {
            shapes.runs.clone()
        };
        Prepared {
            planes: plane_texels(&shapes),
            styles: style_texels(&shapes.styles),
            run_bounds: runs.iter().map(|run| primitive_bounds(&shapes, run.start..run.start + run.len)).collect(),
            set_bounds: shapes.clips.sets.iter().map(|set| set_bounds(&shapes, set)).collect(),
            runs,
            shapes,
        }
    }

    /// The shapes it was made from.
    pub fn shapes(&self) -> &Shapes {
        &self.shapes
    }
}

/// What an upload sends, in the order it sends them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Piece {
    ClipVertices,
    Styles,
    Planes,
    Primitives,
    Atlas,
    Done,
}

/// A page's shapes on their way to the GPU, a step at a time: every part of
/// them, textures a band of rows at a time, so no one frame sends much of a
/// heavy page.
pub struct Upload {
    prepared: Prepared,
    page: Uploaded,
    piece: Piece,
    /// How far into `piece` it has got: bytes of a buffer, rows of a texture.
    sent: usize,
}

impl Upload {
    /// Sends about `budget` more bytes -- at least a piece, however small the
    /// budget -- and says whether everything is there.
    pub fn step(&mut self, gl: &glow::Context, budget: usize) -> bool {
        let mut left = budget.max(1);
        while left > 0 && self.piece != Piece::Done {
            let sent = unsafe { self.send(gl, left) };
            left = left.saturating_sub(sent.max(1));
        }
        self.piece == Piece::Done
    }

    /// Sends up to `most` bytes of the piece under way, at least a row of a
    /// texture, and moves on once it's all there. Says how much it sent.
    unsafe fn send(&mut self, gl: &glow::Context, most: usize) -> usize {
        let shapes = &self.prepared.shapes;
        let (sent, done) = match self.piece {
            Piece::ClipVertices => send_buffer(gl, self.page.clip_vertices, bytemuck::cast_slice(&shapes.clips.vertices), &mut self.sent, most),
            Piece::Styles => send_rows(gl, self.page.styles, &self.prepared.styles, STYLES_WIDTH, &mut self.sent, most),
            Piece::Planes => send_rows(gl, self.page.planes, &self.prepared.planes, PLANES_WIDTH, &mut self.sent, most),
            Piece::Primitives => send_buffer(gl, self.page.instances, bytemuck::cast_slice(&shapes.primitives), &mut self.sent, most),
            Piece::Atlas => {
                // Rows of every layer in turn, a band within one layer at a
                // time: a layer is up to 16 MB.
                let (pages, height) = (&shapes.atlas.pages, shapes.atlas.height() as usize);
                let row = ATLAS_SIZE as usize * 4;
                let total = pages.len() * height;
                if self.sent < total {
                    let (layer, top) = (self.sent / height, self.sent % height);
                    let rows = (most / row).clamp(1, height - top);
                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.page.atlas));
                    let pixels = glow::PixelUnpackData::Slice(Some(&pages[layer][top * row..(top + rows) * row]));
                    gl.tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, 0, 0, top as i32, layer as i32, ATLAS_SIZE as i32, rows as i32, 1, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
                    self.sent += rows;
                    (rows * row, self.sent >= total)
                } else {
                    (0, true)
                }
            }
            Piece::Done => (0, true),
        };
        if done {
            self.piece = match self.piece {
                Piece::ClipVertices => Piece::Styles,
                Piece::Styles => Piece::Planes,
                Piece::Planes => Piece::Primitives,
                Piece::Primitives => Piece::Atlas,
                Piece::Atlas | Piece::Done => Piece::Done,
            };
            self.sent = 0;
        }
        sent
    }

    /// The bytes it uploads in all.
    pub fn bytes(&self) -> usize {
        self.page.bytes
    }

    /// The page uploaded, once `step` has said everything is there.
    pub fn finish(self) -> Uploaded {
        self.page
    }

    /// Abandons the upload, freeing what's been made.
    pub fn destroy(self, gl: &glow::Context) {
        self.page.destroy(gl);
    }
}

/// Sends up to `most` more bytes of `bytes` into `buffer` from `sent` on.
/// Says what it sent and whether it's all there.
unsafe fn send_buffer(gl: &glow::Context, buffer: glow::Buffer, bytes: &[u8], sent: &mut usize, most: usize) -> (usize, bool) {
    let end = sent.saturating_add(most).min(bytes.len());
    if end > *sent {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, *sent as i32, &bytes[*sent..end]);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
    }
    let count = end - *sent;
    *sent = end;
    (count, end >= bytes.len())
}

/// Sends a band of rows of `texels` -- RGBA floats, `width` texels to a row
/// -- into `texture` from row `sent` on: as many as `most` bytes hold, and at
/// least one. Says what it sent and whether it's all there.
unsafe fn send_rows(gl: &glow::Context, texture: glow::Texture, texels: &[f32], width: usize, sent: &mut usize, most: usize) -> (usize, bool) {
    let row = width * 4;
    let total = texels.len() / row;
    let rows = (most / (row * 4)).clamp(1, total.saturating_sub(*sent).max(1));
    if *sent < total {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        let pixels = glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&texels[*sent * row..(*sent + rows) * row])));
        gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, *sent as i32, width as i32, rows as i32, glow::RGBA, glow::FLOAT, pixels);
        gl.bind_texture(glow::TEXTURE_2D, None);
        *sent += rows;
    }
    (rows * row * 4, *sent >= total)
}

/// A page being drawn into an image on the GPU (`Renderer::start_image`).
/// Reading an image back straight after drawing it waits for the GPU to
/// finish everything asked of it so far -- 13 ms on average for a thumbnail
/// while scrolling a drawing set, and 150 ms at worst -- so the pixels are
/// copied into a buffer of their own, and collected once the GPU says
/// they're there.
pub struct PendingImage {
    buffer: glow::Buffer,
    fence: glow::Fence,
    size: [i32; 2],
}

impl PendingImage {
    /// Whether the GPU has drawn it, so `read` won't wait.
    pub fn is_ready(&self, gl: &glow::Context) -> bool {
        let status = unsafe { gl.client_wait_sync(self.fence, 0, 0) };
        status == glow::ALREADY_SIGNALED || status == glow::CONDITION_SATISFIED
    }

    /// The image, rows from the top, colours premultiplied; waits for the GPU
    /// if it isn't drawn yet. `None` if the buffer couldn't be read.
    pub fn read(self, gl: &glow::Context) -> Option<Vec<u8>> {
        let [width, height] = self.size;
        let length = (width * height * 4) as usize;
        unsafe {
            gl.client_wait_sync(self.fence, glow::SYNC_FLUSH_COMMANDS_BIT, i32::MAX);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(self.buffer));
            let mapped = gl.map_buffer_range(glow::PIXEL_PACK_BUFFER, 0, length as i32, glow::MAP_READ_BIT);
            // OpenGL reads its rows from the bottom up.
            let image = (!mapped.is_null()).then(|| {
                let pixels = std::slice::from_raw_parts(mapped, length);
                pixels.chunks_exact(width as usize * 4).rev().flatten().copied().collect()
            });
            if !mapped.is_null() {
                gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
            }
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            self.destroy(gl);
            image
        }
    }

    /// Lets it go, drawn or not.
    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_sync(self.fence);
            gl.delete_buffer(self.buffer);
        }
    }
}

/// How far into a page's shapes drawing has got (`Renderer::paint_some`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Progress {
    run: usize,
    /// Shapes of the run already drawn.
    shape: usize,
}

impl Progress {
    pub const START: Progress = Progress { run: 0, shape: 0 };

    /// Whether every shape of `page` is drawn.
    pub fn is_done(&self, page: &Uploaded) -> bool {
        self.run >= page.runs.len()
    }
}

/// Rough microseconds of work a draw asks of the GPU and its driver, for
/// sharing a page's drawing out over frames. Measured on a drawing set with
/// an RTX laptop GPU: a sheet of 58,833 runs, most clipped through the
/// stencil, took 60 ms, nearly all of it the calls for each run; one of four
/// million shapes in three runs took 19 ms.
pub mod cost {
    /// A run: its draw call and the state set for it.
    pub const RUN: f32 = 0.6;
    /// Drawing a clip set into the stencil before a run.
    pub const CLIP: f32 = 1.2;
    /// A shape of a run.
    pub const SHAPE: f32 = 0.005;
}

/// Somewhere to draw a page off screen, a piece at a time: a multisampled
/// framebuffer with a stencil, as the window's is, and a texture its samples
/// are resolved into once the drawing is done (`Renderer::paint_some`).
pub struct Canvas {
    /// Where it's drawn, until the drawing is done (`keep_only_texture`).
    drawing: Option<Multisampled>,
    resolved: glow::Framebuffer,
    texture: glow::Texture,
    size: [i32; 2],
}

/// A canvas's multisampled framebuffer, and what's attached to it.
struct Multisampled {
    framebuffer: glow::Framebuffer,
    colour: glow::Renderbuffer,
    stencil: glow::Renderbuffer,
    samples: i32,
}

impl Multisampled {
    unsafe fn destroy(self, gl: &glow::Context) {
        gl.delete_framebuffer(self.framebuffer);
        gl.delete_renderbuffer(self.colour);
        gl.delete_renderbuffer(self.stencil);
    }
}

impl Canvas {
    /// A canvas `size` pixels, with `samples` samples a pixel (0 for none).
    /// `None` if the driver won't make one.
    pub fn new(gl: &glow::Context, size: [u32; 2], samples: i32) -> Option<Canvas> {
        let [width, height] = size.map(|side| side.max(1) as i32);
        unsafe {
            let bound = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let texture = texture(gl, glow::TEXTURE_2D, glow::NEAREST).ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width, height, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            gl.bind_texture(glow::TEXTURE_2D, None);
            let (colour, stencil) = (gl.create_renderbuffer().ok()?, gl.create_renderbuffer().ok()?);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(colour));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::RGBA8, width, height);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(stencil));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::DEPTH24_STENCIL8, width, height);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let (drawing, resolved) = (gl.create_framebuffer().ok()?, gl.create_framebuffer().ok()?);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(drawing));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(colour));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(stencil));
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolved));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            let resolvable = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            let was_bound = u32::try_from(bound).ok().and_then(std::num::NonZeroU32::new).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, was_bound);
            let canvas = Canvas { drawing: Some(Multisampled { framebuffer: drawing, colour, stencil, samples }), resolved, texture, size: [width, height] };
            if complete && resolvable {
                Some(canvas)
            } else {
                canvas.destroy(gl);
                None
            }
        }
    }

    /// The texture its drawing is resolved into.
    pub fn texture(&self) -> glow::Texture {
        self.texture
    }

    /// Its size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size.map(|side| side as u32)
    }

    /// Storage for the resolved texture and any drawing attachments still held.
    pub fn bytes(&self) -> usize {
        let pixels = self.size[0] as usize * self.size[1] as usize;
        let samples = self.drawing.as_ref().map_or(0, |d| d.samples.max(1) as usize);
        pixels * (4 + samples * 8)
    }

    /// Lets go of the multisampled framebuffer once the drawing is resolved,
    /// keeping the texture: a finished canvas of 512 pixels a side takes 1 MB
    /// rather than 9.
    pub fn keep_only_texture(&mut self, gl: &glow::Context) {
        if let Some(drawing) = self.drawing.take() {
            unsafe { drawing.destroy(gl) };
        }
    }

    /// Starts reading the resolved drawing back, as `Renderer::start_image`
    /// does, without waiting for the GPU.
    pub fn start_read(&self, gl: &glow::Context) -> Option<PendingImage> {
        let [width, height] = self.size;
        unsafe {
            let bound = gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING);
            let buffer = gl.create_buffer().ok()?;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.resolved));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
            gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, width * height * 4, glow::STREAM_READ);
            gl.read_pixels(0, 0, width, height, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            let was_bound = u32::try_from(bound).ok().and_then(std::num::NonZeroU32::new).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, was_bound);
            match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                Ok(fence) => Some(PendingImage { buffer, fence, size: [width, height] }),
                Err(_) => {
                    gl.delete_buffer(buffer);
                    None
                }
            }
        }
    }

    /// Frees it all.
    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            if let Some(drawing) = self.drawing {
                drawing.destroy(gl);
            }
            gl.delete_framebuffer(self.resolved);
            gl.delete_texture(self.texture);
        }
    }
}

/// One page's shapes on the GPU, ready to draw at any pan and zoom. Its
/// buffers and textures stay until `destroy`.
pub struct Uploaded {
    /// Told apart from every other upload, for whatever is kept of how it
    /// was drawn.
    id: u64,
    instances: glow::Buffer,
    clip_vertices: glow::Buffer,
    planes: glow::Texture,
    styles: glow::Texture,
    atlas: glow::Texture,
    /// How much places in the atlas stretch to fit its texture's rows.
    atlas_scale: [f32; 2],
    count: usize,
    bytes: usize,
    runs: Vec<Run>,
    /// What each run covers, in page points: left, bottom, right, top.
    run_bounds: Vec<[f32; 4]>,
    clip_shapes: Vec<Range<usize>>,
    clip_sets: Vec<Vec<usize>>,
    /// What each clip set can show through, the overlap of its shapes'
    /// bounds, in page points.
    set_bounds: Vec<[f32; 4]>,
}

impl Uploaded {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// A number no other upload in this process has.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Its runs: each a draw, and a clip into the stencil for many.
    pub fn runs(&self) -> usize {
        self.runs.len()
    }

    /// The bytes uploaded for it.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Frees its buffers and textures.
    pub fn destroy(self, gl: &glow::Context) {
        unsafe {
            gl.delete_buffer(self.instances);
            gl.delete_buffer(self.clip_vertices);
            gl.delete_texture(self.planes);
            gl.delete_texture(self.styles);
            gl.delete_texture(self.atlas);
        }
    }
}

impl Renderer {
    /// Whether this context draws in software, with no GPU driver -- where
    /// hundreds of thousands of shapes each frame would be slower than
    /// pictures of them, so they're better drawn another way.
    pub fn is_software(gl: &glow::Context) -> bool {
        is_software_renderer(&unsafe { gl.get_parameter_string(glow::RENDERER) })
    }

    /// Which GPU and driver this context draws with, as their names say.
    pub fn describe(gl: &glow::Context) -> String {
        let [vendor, renderer, version] = [glow::VENDOR, glow::RENDERER, glow::VERSION].map(|name| unsafe { gl.get_parameter_string(name) });
        format!("{renderer} ({vendor}, OpenGL {version})")
    }

    /// Compiles the shaders and lays out the vertices. Needs a current OpenGL
    /// 3.3 or OpenGL ES 3.0 context, with a stencil buffer for clips that
    /// aren't convex.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let shape_program = program(gl, &[TRANSFORM, PLANES, STYLES, SHAPE_VERTEX], &[PLANES, SHAPE_FRAGMENT])?;
            let clip_program = program(gl, &[TRANSFORM, CLIP_VERTEX], &[CLIP_FRAGMENT])?;

            // Six vertices: for a line, x runs 0 to 1 along it and y -1 to 1
            // across; for a triangle, z says which corner.
            let shape_vertex_array = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(shape_vertex_array));
            let corners = gl.create_buffer()?;
            let six: [f32; 18] = [
                0.0, -1.0, 0.0, 1.0, -1.0, 1.0, 1.0, 1.0, 2.0, //
                0.0, -1.0, 3.0, 1.0, 1.0, 4.0, 0.0, 1.0, 5.0,
            ];
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(corners));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&six), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);
            // The shapes themselves, one a draw; each page's own buffer is
            // pointed at as it's drawn.
            for (index, _, _) in ATTRIBUTES {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_divisor(index, 1);
            }
            gl.enable_vertex_attrib_array(STYLE_ATTRIBUTE.0);
            gl.vertex_attrib_divisor(STYLE_ATTRIBUTE.0, 1);

            let clip_vertex_array = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(clip_vertex_array));
            gl.enable_vertex_attrib_array(0);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            let blit_program = program(gl, &[BLIT_VERTEX], &[BLIT_FRAGMENT])?;

            Ok(Renderer {
                blit_box: gl.get_uniform_location(blit_program, "u_box"),
                blit_screen: gl.get_uniform_location(blit_program, "u_screen"),
                blit_sampler: gl.get_uniform_location(blit_program, "u_texture"),
                blit_program,
                blit_vertex_array: gl.create_vertex_array()?,
                shape_transform: Transform::of(gl, shape_program),
                pixels_per_point: gl.get_uniform_location(shape_program, "u_pixels_per_point"),
                pixels_to_page: gl.get_uniform_location(shape_program, "u_pixels_to_page"),
                planes_sampler: gl.get_uniform_location(shape_program, "u_planes"),
                styles_sampler: gl.get_uniform_location(shape_program, "u_styles"),
                atlas_sampler: gl.get_uniform_location(shape_program, "u_atlas"),
                atlas_scale: gl.get_uniform_location(shape_program, "u_atlas_scale"),
                clip_transform: Transform::of(gl, clip_program),
                shape_program,
                shape_vertex_array,
                corners,
                clip_program,
                clip_vertex_array,
                marks: gl.create_buffer()?,
                mark_styles: texture(gl, glow::TEXTURE_2D, glow::NEAREST)?,
            })
        }
    }

    /// Uploads a page's shapes all at once, to draw with `paint` until they're
    /// destroyed.
    pub fn upload(&self, gl: &glow::Context, shapes: Shapes) -> Result<Uploaded, String> {
        let mut upload = self.begin_upload(gl, Prepared::new(shapes))?;
        while !upload.step(gl, usize::MAX) {}
        Ok(upload.finish())
    }

    /// Starts uploading a page's shapes, to go up a step at a time
    /// (`Upload::step`) so no frame waits for all of a heavy page. The
    /// buffers and textures are made now, empty; everything in them is sent
    /// by the steps.
    pub fn begin_upload(&self, gl: &glow::Context, mut prepared: Prepared) -> Result<Upload, String> {
        let shapes = &prepared.shapes;
        // With no atlas pages, one transparent pixel to bind.
        let atlas_width = if shapes.atlas.pages.is_empty() { 1 } else { ATLAS_SIZE as i32 };
        let atlas_height = shapes.atlas.height().max(1) as i32;
        unsafe {
            let page = Uploaded {
                id: {
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                },
                run_bounds: std::mem::take(&mut prepared.run_bounds),
                set_bounds: std::mem::take(&mut prepared.set_bounds),
                runs: std::mem::take(&mut prepared.runs),
                instances: gl.create_buffer()?,
                clip_vertices: gl.create_buffer()?,
                planes: texture(gl, glow::TEXTURE_2D, glow::NEAREST)?,
                styles: texture(gl, glow::TEXTURE_2D, glow::NEAREST)?,
                atlas: texture(gl, glow::TEXTURE_2D_ARRAY, glow::LINEAR)?,
                atlas_scale: [1.0, ATLAS_SIZE as f32 / atlas_height as f32],
                count: prepared.shapes.primitives.len(),
                bytes: prepared.shapes.bytes() + prepared.planes.len() * 4,
                clip_shapes: prepared.shapes.clips.shapes.clone(),
                clip_sets: prepared.shapes.clips.sets.clone(),
            };
            let shapes = &prepared.shapes;
            for (buffer, bytes) in [
                (page.instances, std::mem::size_of_val(shapes.primitives.as_slice())),
                (page.clip_vertices, std::mem::size_of_val(shapes.clips.vertices.as_slice())),
            ] {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
                gl.buffer_data_size(glow::ARRAY_BUFFER, bytes as i32, glow::STATIC_DRAW);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            for (texture, texels, width) in [(page.styles, &prepared.styles, STYLES_WIDTH), (page.planes, &prepared.planes, PLANES_WIDTH)] {
                let rows = (texels.len() / 4 / width) as i32;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, width as i32, rows, 0, glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None));
            }
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(page.atlas));
            let transparent = [0; 4];
            let pixels = glow::PixelUnpackData::Slice(shapes.atlas.pages.is_empty().then_some(&transparent[..]));
            let layers = shapes.atlas.pages.len().max(1) as i32;
            gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::RGBA8 as i32, atlas_width, atlas_height, layers, 0, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(Upload { prepared, page, piece: Piece::ClipVertices, sent: 0 })
        }
    }

    /// Draws `page`'s shapes, then `marks` over them, into the current
    /// viewport, `screen` pixels in size. `page_to_pixels` is the affine map
    /// from page points to pixels in that viewport, origin at its top left:
    /// `[a, b, c, d, e, f]`, taking (x, y) to (a x + c y + e, b x + d y + f).
    /// `pixels_per_point` is how many pixels one point covers.
    pub fn paint(&self, gl: &glow::Context, page: &Uploaded, marks: &[Mark], page_to_pixels: [f32; 6], screen: [f32; 2], pixels_per_point: f32) {
        if page.is_empty() && marks.is_empty() {
            return;
        }
        let page_to_pixels = Matrix(page_to_pixels);
        unsafe {
            self.begin(gl, Some(page), page_to_pixels, screen, pixels_per_point);
            self.draw_runs(gl, page, page_to_pixels, screen, Progress::START, f32::INFINITY, None);
            self.draw_marks(gl, marks);
            self.end(gl);
        }
    }

    /// Draws `marks` alone over what is there, as `paint` draws them over a
    /// page: for a page whose shapes were drawn earlier, into `Canvas`es.
    pub fn paint_marks(&self, gl: &glow::Context, marks: &[Mark], page_to_pixels: [f32; 6], screen: [f32; 2], pixels_per_point: f32) {
        if marks.is_empty() {
            return;
        }
        unsafe {
            self.begin(gl, None, Matrix(page_to_pixels), screen, pixels_per_point);
            self.draw_marks(gl, marks);
            self.end(gl);
        }
    }

    /// Draws more of `page` into `canvas`, as `paint` would draw it into a
    /// view `canvas`'s size through `page_to_pixels`, carrying on from
    /// `from`: until it's all there, or about `budget` microseconds of work
    /// have been asked of the GPU (`cost`). Says how far it got. A canvas is
    /// cleared to paper when `from` is the start.
    ///
    /// A page is drawn in the order its shapes come, so drawing it a piece
    /// at a time into the same canvas comes out the same as all at once --
    /// and a sheet that takes 60 ms to draw can go into a frame a few
    /// milliseconds at a time.
    pub fn paint_some(&self, gl: &glow::Context, page: &Uploaded, canvas: &Canvas, page_to_pixels: [f32; 6], pixels_per_point: f32, from: Progress, budget: f32) -> (Progress, f32) {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs_f32(budget.max(0.0) / 1e6);
        let page_to_pixels = Matrix(page_to_pixels);
        let screen = [canvas.size[0] as f32, canvas.size[1] as f32];
        // A canvas whose drawing is done has nothing left to draw into.
        let Some(drawing) = canvas.drawing.as_ref().map(|d| d.framebuffer) else { return (from, 0.0) };
        unsafe {
            let bound = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            let scissor = gl.is_enabled(glow::SCISSOR_TEST);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(drawing));
            gl.viewport(0, 0, canvas.size[0], canvas.size[1]);
            gl.disable(glow::SCISSOR_TEST);
            if from == Progress::START {
                gl.clear_color(1.0, 1.0, 1.0, 1.0);
                gl.stencil_mask(0xff);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
            }
            self.begin(gl, Some(page), page_to_pixels, screen, pixels_per_point);
            let (reached, spent) = self.draw_runs(gl, page, page_to_pixels, screen, from, budget, Some(deadline));
            self.end(gl);
            // Finished: the samples are resolved into the canvas's texture.
            if reached.is_done(page) {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(drawing));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(canvas.resolved));
                let [w, h] = canvas.size;
                gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            }

            let was_bound = u32::try_from(bound).ok().and_then(std::num::NonZeroU32::new).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, was_bound);
            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            if scissor {
                gl.enable(glow::SCISSOR_TEST);
            }
            (reached, spent)
        }
    }

    /// Sets up the shapes' program to draw through `page_to_pixels`, with
    /// `page`'s textures bound if there's a page.
    unsafe fn begin(&self, gl: &glow::Context, page: Option<&Uploaded>, page_to_pixels: Matrix, screen: [f32; 2], pixels_per_point: f32) {
        let pixels_to_page = page_to_pixels.inverse().unwrap_or(Matrix::IDENTITY);
        gl.use_program(Some(self.clip_program));
        self.clip_transform.set(gl, page_to_pixels, screen);
        gl.use_program(Some(self.shape_program));
        self.shape_transform.set(gl, page_to_pixels, screen);
        gl.uniform_1_f32(self.pixels_per_point.as_ref(), pixels_per_point);
        gl.uniform_matrix_3_f32_slice(self.pixels_to_page.as_ref(), false, &mat3(pixels_to_page));
        gl.uniform_1_i32(self.planes_sampler.as_ref(), 0);
        gl.uniform_1_i32(self.atlas_sampler.as_ref(), 1);
        gl.uniform_1_i32(self.styles_sampler.as_ref(), 2);
        if let Some(page) = page {
            gl.uniform_2_f32(self.atlas_scale.as_ref(), page.atlas_scale[0], page.atlas_scale[1]);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(page.styles));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(page.atlas));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(page.planes));
            self.bind_shapes(gl, page);
        } else {
            gl.bind_vertex_array(Some(self.shape_vertex_array));
        }
        gl.enable(glow::BLEND);
    }

    /// Unbinds what `begin` and the drawing bound.
    unsafe fn end(&self, gl: &glow::Context) {
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_vertex_array(None);
        gl.use_program(None);
    }

    /// Draws `page`'s runs from `from` on, after `begin`, until they're all
    /// drawn or `budget` microseconds of work (`cost`) are spent. Says how far
    /// it got and what it spent.
    unsafe fn draw_runs(&self, gl: &glow::Context, page: &Uploaded, page_to_pixels: Matrix, screen: [f32; 2], from: Progress, budget: f32, deadline: Option<std::time::Instant>) -> (Progress, f32) {
        let stride = std::mem::size_of::<Primitive>() as i32;
        // What's drawn is limited to the scissor box already set -- an
        // egui callback's clip -- or else the viewport, in window pixels
        // from the bottom left. A run outside it is skipped, and a clip
        // is cleared from and drawn into the stencil only where it could
        // show. A drawing sheet's hatches are each clipped to an outline
        // of their own, which put 58,000 runs and a stencil clear of the
        // whole view behind every one on one sheet.
        let scissored = gl.is_enabled(glow::SCISSOR_TEST);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let mut visible = viewport;
        if scissored {
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut visible);
        }
        let to_window = |bounds: [f32; 4]| window_box(bounds, page_to_pixels, viewport, screen);
        gl.enable(glow::SCISSOR_TEST);

        let mut spent = 0.0;
        let mut at = from;
        let mut in_stencil: Option<(usize, [i32; 4])> = None;
        let mut scanned = 0usize;
        while at.run < page.runs.len() {
            // Culled runs still cost CPU time. Check before the culling
            // branches so a mostly offscreen sheet can yield too.
            if scanned % 64 == 0 && deadline.is_some_and(|end| std::time::Instant::now() >= end) {
                break;
            }
            scanned += 1;
            let (run, bounds) = (&page.runs[at.run], page.run_bounds[at.run]);
            let next = Progress { run: at.run + 1, shape: 0 };
            if run.len <= at.shape {
                at = next;
                continue;
            }
            let Some(mut within) = intersect(visible, to_window(bounds)) else {
                at = next;
                continue;
            };
            if spent >= budget {
                break;
            }
            spent += cost::RUN;
            match run.clip {
                Some(set) => {
                    let Some(shown) = intersect(visible, to_window(page.set_bounds[set])) else {
                        at = next;
                        continue;
                    };
                    let Some(both) = intersect(within, shown) else {
                        at = next;
                        continue;
                    };
                    within = both;
                    if in_stencil.map(|(drawn, _)| drawn) != Some(set) {
                        gl.scissor(shown[0], shown[1], shown[2], shown[3]);
                        self.draw_clip_into_stencil(gl, page, set);
                        in_stencil = Some((set, shown));
                        spent += cost::CLIP;
                    }
                    let depth = page.clip_sets[set].len().min(DEEPEST_CLIP) as i32;
                    gl.enable(glow::STENCIL_TEST);
                    gl.stencil_mask(0);
                    gl.stencil_func(glow::EQUAL, depth, 0xff);
                    gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
                }
                None => gl.disable(glow::STENCIL_TEST),
            }
            gl.scissor(within[0], within[1], within[2], within[3]);
            set_blend(gl, run.blend);
            // As many of the run's shapes as the budget has room for.
            let left = run.len - at.shape;
            let room = ((budget - spent).max(0.0) / cost::SHAPE) as usize;
            let count = left.min(room.max(1)).min(i32::MAX as usize);
            // Point the attributes at the first shape to draw. Instanced
            // drawing from an offset needs OpenGL 4.2; moving the pointers
            // works on 3.3 and ES 3.0.
            let base = (run.start + at.shape) as i32 * stride;
            for (index, size, offset) in ATTRIBUTES {
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, base + offset);
            }
            gl.vertex_attrib_pointer_i32(STYLE_ATTRIBUTE.0, 1, glow::UNSIGNED_INT, stride, base + STYLE_ATTRIBUTE.1);
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, count as i32);
            spent += count as f32 * cost::SHAPE;
            at = if count < left { Progress { run: at.run, shape: at.shape + count } } else { next };
        }

        gl.disable(glow::STENCIL_TEST);
        gl.stencil_mask(0xff);
        gl.scissor(visible[0], visible[1], visible[2], visible[3]);
        if !scissored {
            gl.disable(glow::SCISSOR_TEST);
        }
        (at, spent)
    }

    /// Draws `marks` with the shapes' program set up by `begin`.
    unsafe fn draw_marks(&self, gl: &glow::Context, marks: &[Mark]) {
        if marks.is_empty() {
            return;
        }
        let stride = std::mem::size_of::<Primitive>() as i32;
        // Each mark is two triangles of one colour, so one style each.
        let styles: Vec<Style> = marks
            .iter()
            .map(|mark| Style { width: 0.0, kind: 1.0, clip: 0.0, colour: [mark.colour[0], mark.colour[1], mark.colour[2], 1.0] })
            .collect();
        let shapes: Vec<Primitive> = marks
            .iter()
            .enumerate()
            .flat_map(|(mark, &Mark { rect: [left, bottom, right, top], .. })| {
                let style = mark as u32;
                [
                    Primitive { points: [[left, bottom], [right, bottom], [right, top]], style },
                    Primitive { points: [[left, bottom], [right, top], [left, top]], style },
                ]
            })
            .collect();
        upload_styles(gl, self.mark_styles, &styles);
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(self.mark_styles));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.marks));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&shapes), glow::STREAM_DRAW);
        for (index, size, offset) in ATTRIBUTES {
            gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset);
        }
        gl.vertex_attrib_pointer_i32(STYLE_ATTRIBUTE.0, 1, glow::UNSIGNED_INT, stride, STYLE_ATTRIBUTE.1);
        set_blend(gl, Blend::Multiply);
        gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, shapes.len() as i32);
    }

    /// Draws `texture` -- a `Canvas`'s, say -- over `to`, a box of window
    /// pixels from the top left of a view `screen` pixels in size, opaque,
    /// picking the nearest texel when `nearest` or blending between them.
    pub fn blit(&self, gl: &glow::Context, texture: glow::Texture, to: [f32; 4], screen: [f32; 2], nearest: bool) {
        unsafe {
            gl.use_program(Some(self.blit_program));
            gl.uniform_4_f32(self.blit_box.as_ref(), to[0], to[1], to[2], to[3]);
            gl.uniform_2_f32(self.blit_screen.as_ref(), screen[0], screen[1]);
            gl.uniform_1_i32(self.blit_sampler.as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let filter = if nearest { glow::NEAREST } else { glow::LINEAR } as i32;
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
            gl.bind_vertex_array(Some(self.blit_vertex_array));
            gl.disable(glow::BLEND);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.enable(glow::BLEND);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }

    /// Draws the whole of `page` into an image `size` pixels, for a page
    /// `points` in size: a thumbnail, kept so the page can be shown small
    /// without its shapes being on the GPU at all. Rows run from the top,
    /// colours premultiplied, on white paper.
    ///
    /// It draws into a framebuffer of its own and puts back the one that was
    /// bound, so it can be called between frames with the context in hand.
    /// It waits for the GPU; `start_image` doesn't.
    pub fn draw_to_image(&self, gl: &glow::Context, page: &Uploaded, size: [u32; 2], points: [f32; 2]) -> Option<Vec<u8>> {
        self.start_image(gl, page, size, points)?.read(gl)
    }

    /// Starts drawing the whole of `page` into an image, as `draw_to_image`
    /// does, without waiting for the GPU to do it: the image is collected
    /// from what this gives once it `is_ready`. The page's shapes may be let
    /// go meanwhile; OpenGL keeps them until the drawing is done.
    pub fn start_image(&self, gl: &glow::Context, page: &Uploaded, size: [u32; 2], points: [f32; 2]) -> Option<PendingImage> {
        let [width, height] = size.map(|side| side.max(1) as i32);
        unsafe {
            let bound = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            let mut viewport = [0; 4];
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            let scissor = gl.is_enabled(glow::SCISSOR_TEST);

            let paper = texture(gl, glow::TEXTURE_2D, glow::LINEAR).ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(paper));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width, height, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            gl.bind_texture(glow::TEXTURE_2D, None);
            // Clips that aren't convex are drawn through the stencil, so the
            // framebuffer needs one of those too.
            let (frame, stencil) = (gl.create_framebuffer().ok()?, gl.create_renderbuffer().ok()?);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(stencil));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, width, height);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(frame));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(paper), 0);
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(stencil));
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);

            let drawn = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            let mut pending = None;
            if drawn {
                gl.disable(glow::SCISSOR_TEST);
                gl.viewport(0, 0, width, height);
                gl.clear_color(1.0, 1.0, 1.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
                // Page points to pixels, the origin at the top left, as the
                // viewer's own drawing has them.
                let scale = width as f32 / points[0].max(f32::EPSILON);
                self.paint(gl, page, &[], [scale, 0.0, 0.0, -scale, 0.0, points[1] * scale], [width as f32, height as f32], scale);
                // Into a buffer, which the GPU fills when it gets to it.
                if let Ok(buffer) = gl.create_buffer() {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer));
                    gl.buffer_data_size(glow::PIXEL_PACK_BUFFER, width * height * 4, glow::STREAM_READ);
                    gl.read_pixels(0, 0, width, height, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::BufferOffset(0));
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                    match gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                        Ok(fence) => pending = Some(PendingImage { buffer, fence, size: [width, height] }),
                        Err(_) => gl.delete_buffer(buffer),
                    }
                }
            }

            let was_bound = u32::try_from(bound).ok().and_then(std::num::NonZeroU32::new).map(glow::NativeFramebuffer);
            gl.bind_framebuffer(glow::FRAMEBUFFER, was_bound);
            gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
            if scissor {
                gl.enable(glow::SCISSOR_TEST);
            }
            gl.delete_framebuffer(frame);
            gl.delete_renderbuffer(stencil);
            gl.delete_texture(paper);
            pending
        }
    }

    /// Binds the shapes' program and layout, with `page`'s shapes to draw.
    unsafe fn bind_shapes(&self, gl: &glow::Context, page: &Uploaded) {
        gl.use_program(Some(self.shape_program));
        gl.bind_vertex_array(Some(self.shape_vertex_array));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(page.instances));
    }

    /// Draws `page`'s clip set `set` into a cleared stencil: after it, the
    /// stencil holds the number of the set's shapes, in order, that cover each
    /// pixel -- all of them only where every one does. Leaves the shapes'
    /// program bound again.
    unsafe fn draw_clip_into_stencil(&self, gl: &glow::Context, page: &Uploaded, set: usize) {
        gl.use_program(Some(self.clip_program));
        gl.bind_vertex_array(Some(self.clip_vertex_array));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(page.clip_vertices));
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
        gl.enable(glow::STENCIL_TEST);
        gl.color_mask(false, false, false, false);
        gl.stencil_mask(0xff);
        gl.clear_stencil(0);
        gl.clear(glow::STENCIL_BUFFER_BIT);
        for (level, &shape) in page.clip_sets[set].iter().enumerate().take(DEEPEST_CLIP) {
            let vertices = &page.clip_shapes[shape];
            gl.stencil_func(glow::EQUAL, level as i32, 0xff);
            gl.stencil_op(glow::KEEP, glow::KEEP, glow::INCR);
            gl.draw_arrays(glow::TRIANGLES, vertices.start as i32, vertices.len() as i32);
        }
        gl.color_mask(true, true, true, true);
        self.bind_shapes(gl, page);
    }

    /// Frees the shaders and layouts. Pages uploaded are freed apart.
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.shape_program);
            gl.delete_program(self.clip_program);
            gl.delete_vertex_array(self.shape_vertex_array);
            gl.delete_vertex_array(self.clip_vertex_array);
            gl.delete_buffer(self.corners);
            gl.delete_buffer(self.marks);
            gl.delete_program(self.blit_program);
            gl.delete_vertex_array(self.blit_vertex_array);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Shape;

    #[test]
    fn runs_are_culled_by_what_they_cover_on_screen() {
        // A 100 x 100 point page drawn twice its size, its top at the top of a
        // 200 x 200 pixel viewport that sits 10 pixels up the window.
        let page_to_pixels = Matrix([2.0, 0.0, 0.0, -2.0, 0.0, 200.0]);
        let viewport = [0, 10, 200, 200];
        let mut shapes = Shapes::default();
        shapes.push(Shape::line([10.0, 10.0], [20.0, 10.0], 4.0, [0.0; 4]), Blend::Normal, None);
        shapes.push(Shape::triangle([[150.0, 150.0], [160.0, 150.0], [150.0, 160.0]], [0.0; 4]), Blend::Normal, None);

        // The line reaches half its width past its points.
        let line = primitive_bounds(&shapes, 0..1);
        assert_eq!(line, [8.0, 8.0, 22.0, 12.0]);
        // Rows count up from the window's bottom, widened by the margin.
        assert_eq!(window_box(line, page_to_pixels, viewport, [200.0, 200.0]), [14, 24, 32, 12]);

        let off_page = window_box(primitive_bounds(&shapes, 1..2), page_to_pixels, viewport, [200.0, 200.0]);
        assert_eq!(intersect(viewport, off_page), None);
        assert_eq!(intersect([0, 0, 10, 10], [5, 5, 10, 10]), Some([5, 5, 5, 5]));
        assert_eq!(window_box([1.0, 1.0, 0.0, 0.0], page_to_pixels, viewport, [200.0, 200.0]), [0, 0, 0, 0], "empty bounds");
    }

    #[test]
    fn software_renderers_are_told_from_gpus() {
        for software in ["llvmpipe (LLVM 17.0.6, 256 bits)", "Microsoft Basic Render Driver", "GDI Generic", "Google SwiftShader"] {
            assert!(is_software_renderer(software), "{software}");
        }
        for gpu in ["AMD Radeon 780M Graphics", "Intel(R) Iris(R) Xe Graphics", "NVIDIA GeForce RTX 4060/PCIe/SSE2", "Apple M2"] {
            assert!(!is_software_renderer(gpu), "{gpu}");
        }
    }

    #[test]
    fn clip_texels_give_each_set_its_planes_then_the_planes() {
        let mut shapes = Shapes::default();
        let square = [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]], [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0]]];
        let l = [[[0.0, 0.0], [2.0, 0.0], [2.0, 4.0]], [[0.0, 0.0], [2.0, 4.0], [0.0, 4.0]], [[2.0, 0.0], [4.0, 0.0], [4.0, 2.0]], [[2.0, 0.0], [4.0, 2.0], [2.0, 2.0]]];
        shapes.clips.intersect(None, &square);
        shapes.clips.intersect(None, &l);
        let texels = plane_texels(&shapes);
        assert_eq!(texels.len() % (PLANES_WIDTH * 4), 0, "whole rows");
        assert_eq!(&texels[..8], &[2.0, 4.0, 0.0, 0.0, 6.0, 0.0, 0.0, 0.0], "the square's four planes start after the two sets; the L has none");
        let [a, b, c] = shapes.clips.planes[0].as_ref().unwrap()[0];
        assert_eq!(&texels[8..12], &[a, b, c, 0.0]);
    }
}
