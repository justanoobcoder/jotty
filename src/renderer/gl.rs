//! OpenGL ES 3 rendering on a Wayland EGL surface.

use std::collections::HashMap;
use std::ffi::c_void;

use glow::HasContext;
use khronos_egl as egl;
use wayland_client::backend::ObjectId;
use wayland_egl::WlEglSurface;

use super::atlas::Packer;
use super::{Batch, Frame, GlyphCache, GlyphInfo, Theme, VERTEX_FLOATS};
use crate::font::{CellMetrics, Fonts, Style};

const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;
const ATLAS_SIZE: u32 = 2048;

const VERTEX_SHADER: &str = r#"#version 300 es
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_color;
layout(location = 3) in float a_mode;
uniform vec2 u_viewport;
out vec2 v_uv;
out vec4 v_color;
out float v_mode;
void main() {
    vec2 ndc = a_pos / u_viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
    v_mode = a_mode;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;
in vec2 v_uv;
in vec4 v_color;
in float v_mode;
uniform sampler2D u_atlas;
out vec4 o_color;
void main() {
    float alpha = v_color.a;
    if (v_mode > 0.5) {
        alpha *= texture(u_atlas, v_uv).r;
    }
    // Premultiplied alpha.
    o_color = vec4(v_color.rgb * alpha, alpha);
}
"#;

/// The EGL display, context and window surface.
struct Egl {
    api: &'static egl::Instance<egl::Static>,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
    window: WlEglSurface,
}

impl Egl {
    /// # Safety
    /// `display_ptr` must be a live `wl_display` and `surface` a live
    /// `wl_surface` on it, both outliving the returned value.
    unsafe fn new(
        display_ptr: *mut c_void,
        surface: ObjectId,
        width: i32,
        height: i32,
    ) -> Result<Egl, String> {
        let api = &egl::API;
        let display = unsafe {
            api.get_platform_display(PLATFORM_WAYLAND_KHR, display_ptr, &[egl::ATTRIB_NONE])
        }
        .map_err(|e| format!("eglGetPlatformDisplay: {e}"))?;
        api.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        api.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;

        let config_attribs = [
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = api
            .choose_first_config(display, &config_attribs)
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or("no EGL config supports OpenGL ES 3 with alpha")?;
        let context = api
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE],
            )
            .map_err(|e| format!("eglCreateContext: {e}"))?;

        let window = WlEglSurface::new(surface, width.max(1), height.max(1))
            .map_err(|e| format!("wl_egl_window: {e}"))?;
        let surface = unsafe {
            api.create_window_surface(display, config, window.ptr() as egl::NativeWindowType, None)
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        api.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // Never block in eglSwapBuffers; frames are paced by frame callbacks.
        if let Err(e) = api.swap_interval(display, 0) {
            log::warn!("eglSwapInterval: {e}");
        }
        Ok(Egl {
            api,
            display,
            context,
            surface,
            window,
        })
    }
}

impl Drop for Egl {
    fn drop(&mut self) {
        let _ = self.api.make_current(self.display, None, None, None);
        let _ = self.api.destroy_surface(self.display, self.surface);
        let _ = self.api.destroy_context(self.display, self.context);
        let _ = self.api.terminate(self.display);
    }
}

/// Glyph bitmaps packed into one single-channel texture.
struct Atlas {
    texture: glow::Texture,
    packer: Packer,
    glyphs: HashMap<(char, Style), Option<GlyphInfo>>,
    /// The atlas filled up during this frame and was cleared.
    overflowed: bool,
}

/// Rasterizes and uploads glyphs on demand while a frame is built.
struct Uploader<'a> {
    gl: &'a glow::Context,
    atlas: &'a mut Atlas,
    fonts: &'a mut Fonts,
}

impl GlyphCache for Uploader<'_> {
    fn glyph(&mut self, c: char, style: Style) -> Option<GlyphInfo> {
        if let Some(info) = self.atlas.glyphs.get(&(c, style)) {
            return *info;
        }
        let info = self.fonts.rasterize(c, style).and_then(|glyph| {
            let (x, y) = match self.atlas.packer.alloc(glyph.width, glyph.height) {
                Some(pos) => pos,
                None => {
                    // Start over; glyphs placed earlier this frame are stale,
                    // so the caller redraws once more.
                    self.atlas.packer.clear();
                    self.atlas.glyphs.clear();
                    self.atlas.overflowed = true;
                    self.atlas.packer.alloc(glyph.width, glyph.height)?
                }
            };
            unsafe {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.atlas.texture));
                self.gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    x as i32,
                    y as i32,
                    glyph.width as i32,
                    glyph.height as i32,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&glyph.data)),
                );
            }
            let (aw, ah) = self.atlas.packer.size();
            let (aw, ah) = (aw as f32, ah as f32);
            Some(GlyphInfo {
                uv: [
                    x as f32 / aw,
                    y as f32 / ah,
                    (x + glyph.width) as f32 / aw,
                    (y + glyph.height) as f32 / ah,
                ],
                width: glyph.width,
                height: glyph.height,
                left: glyph.left,
                top: glyph.top,
            })
        });
        self.atlas.glyphs.insert((c, style), info);
        info
    }
}

pub struct Renderer {
    // Field order matters: GL objects must go before the EGL context.
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    viewport_uniform: Option<glow::UniformLocation>,
    atlas: Atlas,
    egl: Egl,
    pub fonts: Fonts,
    pub theme: Theme,
    size: (u32, u32),
}

unsafe fn compile(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl.create_shader(kind)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            return Err(gl.get_shader_info_log(shader));
        }
        Ok(shader)
    }
}

impl Renderer {
    /// # Safety
    /// `display_ptr` must be the connection's live `wl_display` and
    /// `surface` a live `wl_surface`, both outliving the renderer.
    pub unsafe fn new(
        display_ptr: *mut c_void,
        surface: ObjectId,
        size: (u32, u32),
        fonts: Fonts,
        theme: Theme,
    ) -> Result<Renderer, String> {
        let egl = unsafe { Egl::new(display_ptr, surface, size.0 as i32, size.1 as i32)? };
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.api
                    .get_proc_address(name)
                    .map_or(std::ptr::null(), |f| f as *const c_void)
            })
        };
        log::info!("OpenGL: {}", unsafe {
            gl.get_parameter_string(glow::VERSION)
        });

        unsafe {
            let program = gl.create_program()?;
            let vs = compile(&gl, glow::VERTEX_SHADER, VERTEX_SHADER)?;
            let fs = compile(&gl, glow::FRAGMENT_SHADER, FRAGMENT_SHADER)?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            gl.use_program(Some(program));
            let viewport_uniform = gl.get_uniform_location(program, "u_viewport");
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_atlas").as_ref(), 0);

            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (VERTEX_FLOATS * 4) as i32;
            for (index, size, offset) in [(0, 2, 0), (1, 2, 2), (2, 4, 4), (3, 1, 8)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset * 4);
            }

            let texture = gl.create_texture()?;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R8 as i32,
                ATLAS_SIZE as i32,
                ATLAS_SIZE as i32,
                0,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            // Glyphs are drawn at integer pixel positions, 1:1.
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }

            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            Ok(Renderer {
                gl,
                program,
                vao,
                vbo,
                viewport_uniform,
                atlas: Atlas {
                    texture,
                    packer: Packer::new(ATLAS_SIZE, ATLAS_SIZE),
                    glyphs: HashMap::new(),
                    overflowed: false,
                },
                egl,
                fonts,
                theme,
                size,
            })
        }
    }

    pub fn cell(&self) -> CellMetrics {
        self.fonts.metrics
    }

    /// Resizes the drawable, in physical pixels.
    pub fn resize(&mut self, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            self.egl
                .window
                .resize(size.0.max(1) as i32, size.1.max(1) as i32, 0, 0);
        }
    }

    pub fn set_font_size(&mut self, px: f32) {
        self.fonts.set_size(px);
        self.atlas.packer.clear();
        self.atlas.glyphs.clear();
    }

    /// Draws and presents a frame (this commits the surface). Returns
    /// whether another frame is needed because the glyph atlas overflowed.
    pub fn draw(&mut self, frame: &Frame) -> Result<bool, String> {
        self.atlas.overflowed = false;
        let metrics = self.fonts.metrics;
        let batch: Batch = {
            let mut uploader = Uploader {
                gl: &self.gl,
                atlas: &mut self.atlas,
                fonts: &mut self.fonts,
            };
            super::build(frame, &self.theme, metrics, &mut uploader)
        };

        let gl = &self.gl;
        let (w, h) = self.size;
        let [r, g, b] = self.theme.background.to_f32();
        let a = self.theme.opacity;
        unsafe {
            gl.viewport(0, 0, w as i32, h as i32);
            gl.clear_color(r * a, g * a, b * a, a);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.uniform_2_f32(self.viewport_uniform.as_ref(), w as f32, h as f32);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let bytes = std::slice::from_raw_parts(
                batch.vertices.as_ptr().cast::<u8>(),
                batch.vertices.len() * 4,
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas.texture));
            gl.draw_arrays(
                glow::TRIANGLES,
                0,
                (batch.vertices.len() / VERTEX_FLOATS) as i32,
            );
        }
        self.egl
            .api
            .swap_buffers(self.egl.display, self.egl.surface)
            .map_err(|e| format!("eglSwapBuffers: {e}"))?;
        Ok(self.atlas.overflowed)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_texture(self.atlas.texture);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
        }
    }
}
