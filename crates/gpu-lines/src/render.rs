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
use crate::{Blend, Primitive, Run, Shapes};

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

const SHAPE_VERTEX: &str = r#"
layout(location = 0) in vec3 a_corner;
layout(location = 1) in vec2 a_p0;
layout(location = 2) in vec2 a_p1;
layout(location = 3) in vec2 a_p2;
layout(location = 4) in float a_width;
layout(location = 5) in float a_kind;
layout(location = 6) in vec4 a_colour;
layout(location = 7) in float a_clip;

uniform float u_pixels_per_point;
uniform mat3 u_pixels_to_page;

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
    vec2 position;
    int kind = int(a_kind + 0.5);
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
        v_uv = vec2(mix(a_colour.x, a_colour.z, right), mix(a_colour.w, a_colour.y, up));
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

        // Hairlines, and lines thinner than a pixel, are drawn a pixel wide;
        // thinner ones fade rather than vanish, as a rasteriser's coverage
        // would.
        float wanted = a_width * u_pixels_per_point;
        float width = max(wanted, 1.0);
        float fade = a_width == 0.0 ? 1.0 : clamp(wanted, 0.3, 1.0);

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
        v_colour = vec4(a_colour.rgb, a_colour.a * fade);
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

/// Where each per-shape attribute sits in a `Primitive`: attribute index,
/// number of floats, byte offset.
const ATTRIBUTES: [(u32, i32, i32); 7] = [(1, 2, 0), (2, 2, 8), (3, 2, 16), (4, 1, 24), (5, 1, 28), (6, 4, 32), (7, 1, 48)];

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

/// Shapes uploaded to the GPU, ready to draw at any pan and zoom.
pub struct Renderer {
    shape_program: glow::Program,
    shape_transform: Transform,
    pixels_per_point: Option<glow::UniformLocation>,
    pixels_to_page: Option<glow::UniformLocation>,
    planes_sampler: Option<glow::UniformLocation>,
    atlas_sampler: Option<glow::UniformLocation>,
    shape_vertex_array: glow::VertexArray,
    corners: glow::Buffer,
    instances: glow::Buffer,
    planes: glow::Texture,
    atlas: glow::Texture,
    clip_program: glow::Program,
    clip_transform: Transform,
    clip_vertex_array: glow::VertexArray,
    clip_vertices: glow::Buffer,
    count: usize,
    runs: Vec<Run>,
    clip_shapes: Vec<Range<usize>>,
    clip_sets: Vec<Vec<usize>>,
}

impl Renderer {
    /// Compiles the shaders and makes the buffers. Needs a current OpenGL 3.3
    /// or OpenGL ES 3.0 context, with a stencil buffer for clips that aren't
    /// convex.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let shape_program = program(gl, &[TRANSFORM, PLANES, SHAPE_VERTEX], &[PLANES, SHAPE_FRAGMENT])?;
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
            let instances = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(instances));
            for (index, _, _) in ATTRIBUTES {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_divisor(index, 1);
            }

            let clip_vertex_array = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(clip_vertex_array));
            let clip_vertices = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(clip_vertices));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Renderer {
                shape_transform: Transform::of(gl, shape_program),
                pixels_per_point: gl.get_uniform_location(shape_program, "u_pixels_per_point"),
                pixels_to_page: gl.get_uniform_location(shape_program, "u_pixels_to_page"),
                planes_sampler: gl.get_uniform_location(shape_program, "u_planes"),
                atlas_sampler: gl.get_uniform_location(shape_program, "u_atlas"),
                clip_transform: Transform::of(gl, clip_program),
                shape_program,
                shape_vertex_array,
                corners,
                instances,
                planes: texture(gl, glow::TEXTURE_2D, glow::NEAREST)?,
                atlas: texture(gl, glow::TEXTURE_2D_ARRAY, glow::LINEAR)?,
                clip_program,
                clip_vertex_array,
                clip_vertices,
                count: 0,
                runs: Vec::new(),
                clip_shapes: Vec::new(),
                clip_sets: Vec::new(),
            })
        }
    }

    /// Replaces what's drawn. Returns the bytes uploaded.
    pub fn upload(&mut self, gl: &glow::Context, shapes: &Shapes) -> usize {
        let primitives: &[u8] = bytemuck::cast_slice(&shapes.primitives);
        let clip_vertices: &[u8] = bytemuck::cast_slice(&shapes.clips.vertices);
        let texels = plane_texels(shapes);
        // The atlas's pages; or with none, a transparent pixel to bind.
        let (side, pages, atlas) = match shapes.atlas.pages.len() {
            0 => (1, 1, vec![0; 4]),
            pages => (ATLAS_SIZE as i32, pages as i32, shapes.atlas.pages.concat()),
        };
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, primitives, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.clip_vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, clip_vertices, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.planes));
            let rows = (texels.len() / 4 / PLANES_WIDTH) as i32;
            let pixels = glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&texels)));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, PLANES_WIDTH as i32, rows, 0, glow::RGBA, glow::FLOAT, pixels);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.atlas));
            let pixels = glow::PixelUnpackData::Slice(Some(&atlas));
            gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::RGBA8 as i32, side, side, pages, 0, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        }
        self.count = shapes.primitives.len();
        self.runs = if shapes.runs.is_empty() {
            vec![Run { start: 0, len: shapes.primitives.len(), blend: Blend::Normal, clip: None }]
        } else {
            shapes.runs.clone()
        };
        self.clip_shapes = shapes.clips.shapes.clone();
        self.clip_sets = shapes.clips.sets.clone();
        primitives.len() + clip_vertices.len() + texels.len() * 4 + atlas.len()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Draws the shapes into the current viewport, `screen` pixels in size.
    /// `page_to_pixels` is the affine map from page points to pixels in that
    /// viewport, origin at its top left: `[a, b, c, d, e, f]`, taking (x, y)
    /// to (a x + c y + e, b x + d y + f). `pixels_per_point` is how many pixels
    /// one point covers.
    pub fn paint(&self, gl: &glow::Context, page_to_pixels: [f32; 6], screen: [f32; 2], pixels_per_point: f32) {
        if self.count == 0 {
            return;
        }
        let page_to_pixels = Matrix(page_to_pixels);
        let pixels_to_page = page_to_pixels.inverse().unwrap_or(Matrix::IDENTITY);
        let stride = std::mem::size_of::<Primitive>() as i32;
        unsafe {
            gl.use_program(Some(self.clip_program));
            self.clip_transform.set(gl, page_to_pixels, screen);
            gl.use_program(Some(self.shape_program));
            self.shape_transform.set(gl, page_to_pixels, screen);
            gl.uniform_1_f32(self.pixels_per_point.as_ref(), pixels_per_point);
            gl.uniform_matrix_3_f32_slice(self.pixels_to_page.as_ref(), false, &mat3(pixels_to_page));
            gl.uniform_1_i32(self.planes_sampler.as_ref(), 0);
            gl.uniform_1_i32(self.atlas_sampler.as_ref(), 1);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.atlas));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.planes));
            self.bind_shapes(gl);
            gl.enable(glow::BLEND);

            let mut in_stencil: Option<usize> = None;
            for run in self.runs.iter().filter(|r| r.len > 0) {
                match run.clip {
                    Some(set) => {
                        if in_stencil != Some(set) {
                            self.draw_clip_into_stencil(gl, set);
                            in_stencil = Some(set);
                        }
                        let depth = self.clip_sets[set].len().min(DEEPEST_CLIP) as i32;
                        gl.enable(glow::STENCIL_TEST);
                        gl.stencil_mask(0);
                        gl.stencil_func(glow::EQUAL, depth, 0xff);
                        gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
                    }
                    None => gl.disable(glow::STENCIL_TEST),
                }
                match run.blend {
                    // Colours are premultiplied by alpha.
                    Blend::Normal => gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE),
                    // Over an opaque page: source times what's there, plus
                    // what's there where the source is see-through.
                    Blend::Multiply => gl.blend_func_separate(glow::DST_COLOR, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE),
                }
                // Point the attributes at this run's first shape. Instanced
                // drawing from an offset needs OpenGL 4.2; moving the pointers
                // works on 3.3 and ES 3.0.
                let base = run.start as i32 * stride;
                for (index, size, offset) in ATTRIBUTES {
                    gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, base + offset);
                }
                gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, run.len.min(i32::MAX as usize) as i32);
            }

            gl.disable(glow::STENCIL_TEST);
            gl.stencil_mask(0xff);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    unsafe fn bind_shapes(&self, gl: &glow::Context) {
        gl.use_program(Some(self.shape_program));
        gl.bind_vertex_array(Some(self.shape_vertex_array));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
    }

    /// Draws clip set `set` into a cleared stencil: after it, the stencil
    /// holds the number of the set's shapes, in order, that cover each pixel
    /// -- all of them only where every one does. Leaves the shapes' program
    /// bound again.
    unsafe fn draw_clip_into_stencil(&self, gl: &glow::Context, set: usize) {
        gl.use_program(Some(self.clip_program));
        gl.bind_vertex_array(Some(self.clip_vertex_array));
        gl.enable(glow::STENCIL_TEST);
        gl.color_mask(false, false, false, false);
        gl.stencil_mask(0xff);
        gl.clear_stencil(0);
        gl.clear(glow::STENCIL_BUFFER_BIT);
        for (level, &shape) in self.clip_sets[set].iter().enumerate().take(DEEPEST_CLIP) {
            let vertices = &self.clip_shapes[shape];
            gl.stencil_func(glow::EQUAL, level as i32, 0xff);
            gl.stencil_op(glow::KEEP, glow::KEEP, glow::INCR);
            gl.draw_arrays(glow::TRIANGLES, vertices.start as i32, vertices.len() as i32);
        }
        gl.color_mask(true, true, true, true);
        self.bind_shapes(gl);
    }

    /// Frees the GPU's copies.
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.shape_program);
            gl.delete_program(self.clip_program);
            gl.delete_vertex_array(self.shape_vertex_array);
            gl.delete_vertex_array(self.clip_vertex_array);
            gl.delete_buffer(self.corners);
            gl.delete_buffer(self.instances);
            gl.delete_buffer(self.clip_vertices);
            gl.delete_texture(self.planes);
            gl.delete_texture(self.atlas);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
