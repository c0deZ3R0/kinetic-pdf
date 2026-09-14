//! Drawing shapes with OpenGL. Every shape -- a piece of line or a filled
//! triangle -- is an instance of the same six vertices, so a page of a million
//! shapes is one buffer upload, then a draw call per run of shapes sharing a
//! blend, painted in the page's own order. A line's six vertices make a quad
//! stretched between its ends and widened in the vertex shader; a triangle
//! uses three of them for its corners and folds the other three away.

use glow::HasContext;

use crate::{Blend, Primitive, Run};

/// Shapes uploaded to the GPU, ready to draw at any pan and zoom.
pub struct Renderer {
    program: glow::Program,
    vertex_array: glow::VertexArray,
    corners: glow::Buffer,
    instances: glow::Buffer,
    count: usize,
    runs: Vec<Run>,
    page_to_pixels: Option<glow::UniformLocation>,
    screen: Option<glow::UniformLocation>,
    pixels_per_point: Option<glow::UniformLocation>,
}

const VERTEX: &str = r#"
layout(location = 0) in vec3 a_corner;
layout(location = 1) in vec2 a_p0;
layout(location = 2) in vec2 a_p1;
layout(location = 3) in vec2 a_p2;
layout(location = 4) in float a_width;
layout(location = 5) in float a_kind;
layout(location = 6) in vec4 a_colour;

uniform mat3 u_page_to_pixels;
uniform vec2 u_screen;
uniform float u_pixels_per_point;

out vec4 v_colour;
out float v_across;
out float v_half;

vec2 to_pixels(vec2 p) {
    return (u_page_to_pixels * vec3(p, 1.0)).xy;
}

void main() {
    vec2 position;
    if (a_kind > 0.5) {
        // A triangle: corners 0, 1 and 2, the rest folded onto corner 0.
        int corner = int(a_corner.z + 0.5);
        position = to_pixels(corner == 1 ? a_p1 : corner == 2 ? a_p2 : a_p0);
        v_across = 0.0;
        v_half = 1.0e6;
        v_colour = a_colour;
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

        // A pixel of fringe each side for anti-aliasing, and half a pixel past
        // each end so the pieces of a flattened curve meet.
        float half_width = width * 0.5 + 1.0;
        position = p0 + along * (a_corner.x * (len + 1.0) - 0.5) + across * (a_corner.y * half_width);
        v_across = a_corner.y * half_width;
        v_half = width * 0.5;
        v_colour = vec4(a_colour.rgb, a_colour.a * fade);
    }
    gl_Position = vec4(position.x / u_screen.x * 2.0 - 1.0, 1.0 - position.y / u_screen.y * 2.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
in vec4 v_colour;
in float v_across;
in float v_half;
out vec4 frag_colour;

void main() {
    float coverage = clamp(v_half + 0.5 - abs(v_across), 0.0, 1.0);
    float alpha = v_colour.a * coverage;
    frag_colour = vec4(v_colour.rgb * alpha, alpha);
}
"#;

/// Where each per-shape attribute sits in a `Primitive`: attribute index,
/// number of floats, byte offset.
const ATTRIBUTES: [(u32, i32, i32); 6] = [(1, 2, 0), (2, 2, 8), (3, 2, 16), (4, 1, 24), (5, 1, 28), (6, 4, 32)];

/// The shader's first lines for this context: GLSL 3.30 on desktop OpenGL,
/// 3.00 ES on OpenGL ES and WebGL 2. Instanced drawing needs one of those.
fn header(gl: &glow::Context) -> &'static str {
    let version = unsafe { gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION) };
    if version.contains("ES") {
        "#version 300 es\nprecision highp float;\n"
    } else {
        "#version 330 core\n"
    }
}

impl Renderer {
    /// Compiles the shaders and makes the buffers. Needs a current OpenGL 3.3
    /// or OpenGL ES 3.0 context.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{}{source}", header(gl)));
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

            let vertex_array = gl.create_vertex_array()?;
            gl.bind_vertex_array(Some(vertex_array));

            // Six vertices: for a line, x runs 0 to 1 along it and y -1 to 1
            // across; for a triangle, z says which corner.
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
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Renderer {
                page_to_pixels: gl.get_uniform_location(program, "u_page_to_pixels"),
                screen: gl.get_uniform_location(program, "u_screen"),
                pixels_per_point: gl.get_uniform_location(program, "u_pixels_per_point"),
                program,
                vertex_array,
                corners,
                instances,
                count: 0,
                runs: Vec::new(),
            })
        }
    }

    /// Replaces the shapes drawn, and the runs they're blended in (none means
    /// one normal run of them all). Returns the bytes uploaded.
    pub fn upload(&mut self, gl: &glow::Context, primitives: &[Primitive], runs: &[Run]) -> usize {
        let bytes: &[u8] = bytemuck::cast_slice(primitives);
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        self.count = primitives.len();
        self.runs = if runs.is_empty() { vec![Run { start: 0, len: primitives.len(), blend: Blend::Normal }] } else { runs.to_vec() };
        bytes.len()
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
    /// one point of stroke width covers.
    pub fn paint(&self, gl: &glow::Context, page_to_pixels: [f32; 6], screen: [f32; 2], pixels_per_point: f32) {
        if self.count == 0 {
            return;
        }
        let [a, b, c, d, e, f] = page_to_pixels;
        let stride = std::mem::size_of::<Primitive>() as i32;
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_matrix_3_f32_slice(self.page_to_pixels.as_ref(), false, &[a, b, 0.0, c, d, 0.0, e, f, 1.0]);
            gl.uniform_2_f32(self.screen.as_ref(), screen[0], screen[1]);
            gl.uniform_1_f32(self.pixels_per_point.as_ref(), pixels_per_point);
            gl.enable(glow::BLEND);
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.instances));
            for run in self.runs.iter().filter(|r| r.len > 0) {
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
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Frees the GPU's copies.
    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vertex_array);
            gl.delete_buffer(self.corners);
            gl.delete_buffer(self.instances);
        }
    }
}
