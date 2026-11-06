//! OpenGL ES 3 rendering on a Wayland EGL surface.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;

use glow::HasContext;
use khronos_egl as egl;
use wayland_client::backend::ObjectId;
use wayland_egl::WlEglSurface;

use super::atlas::Atlas;
use super::{Batch, Frame, Theme, VERTEX_FLOATS};
use crate::font::{CellMetrics, Fonts, Glyph};

const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;
const ATLAS_SIZE: u32 = 2048;
/// The color emoji atlas (RGBA).
const COLOR_ATLAS_SIZE: u32 = 1024;

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
uniform sampler2D u_color;
uniform sampler2D u_image;
out vec4 o_color;
void main() {
    if (v_mode > 2.5) {
        // An image, straight alpha.
        vec4 c = texture(u_image, v_uv);
        o_color = vec4(c.rgb * c.a, c.a) * v_color.a;
        return;
    }
    if (v_mode > 1.5) {
        // Color emoji, already premultiplied.
        o_color = texture(u_color, v_uv) * v_color.a;
        return;
    }
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
        // SAFETY: `display_ptr` is a live `wl_display`, as this function requires.
        let display = unsafe {
            api.get_platform_display(PLATFORM_WAYLAND_KHR, display_ptr, &[egl::ATTRIB_NONE])
        }
        .map_err(|e| format!("eglGetPlatformDisplay: {e}"))?;
        api.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        // SAFETY: the same promises this function requires of its caller.
        let egl = unsafe { Self::on_display(api, display, surface, width, height) };
        if egl.is_err() {
            // Also destroys the context and surface made so far, as the CPU
            // renderer takes over for the rest of the process.
            if let Err(e) = api.terminate(display) {
                log::debug!("eglTerminate: {e}");
            }
        }
        egl
    }

    /// # Safety
    /// As for `new`; `display` is initialized.
    unsafe fn on_display(
        api: &'static egl::Instance<egl::Static>,
        display: egl::Display,
        surface: ObjectId,
        width: i32,
        height: i32,
    ) -> Result<Egl, String> {
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
        // SAFETY: `window` is a live `wl_egl_window` that `Egl` keeps until the surface is gone.
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

pub struct Renderer {
    // Field order matters: GL objects must go before the EGL context.
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    viewport_uniform: Option<glow::UniformLocation>,
    texture: glow::Texture,
    color_texture: glow::Texture,
    /// Textures of the images drawn so far, by uid.
    images: HashMap<u64, glow::Texture>,
    max_texture_size: u32,
    /// Images too big for a texture, already reported.
    oversized_images: HashSet<u64>,
    atlas: Atlas,
    egl: Egl,
    pub fonts: Fonts,
    pub theme: Theme,
    size: (u32, u32),
}

unsafe fn compile(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, String> {
    // SAFETY: the caller's GL context is current, and only objects made here are used.
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

/// Creates an empty `size`×`size` texture on texture unit `unit`.
/// Glyphs are drawn at integer pixel positions, 1:1, so it is sampled with
/// `NEAREST`.
unsafe fn create_texture(
    gl: &glow::Context,
    unit: u32,
    internal_format: u32,
    format: u32,
    size: u32,
) -> Result<glow::Texture, String> {
    // SAFETY: the caller's GL context is current, and only objects made here are used.
    unsafe {
        let texture = gl.create_texture()?;
        gl.active_texture(unit);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            internal_format as i32,
            size as i32,
            size as i32,
            0,
            format,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (param, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        Ok(texture)
    }
}

/// Uploads an image as a texture on texture unit 2. Images are scaled, so
/// they are sampled with `LINEAR`.
unsafe fn upload_image(
    gl: &glow::Context,
    image: &crate::terminal::graphics::Image,
) -> Result<glow::Texture, String> {
    // SAFETY: the caller's GL context is current; `image.pixels` holds width × height RGBA
    // pixels, which `decode` checked.
    unsafe {
        let texture = gl.create_texture()?;
        gl.active_texture(glow::TEXTURE2);
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            image.width as i32,
            image.height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&image.pixels)),
        );
        for (param, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        Ok(texture)
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
        // SAFETY: this function's caller promises what `Egl::new` requires.
        let egl = unsafe { Egl::new(display_ptr, surface, size.0 as i32, size.1 as i32)? };
        // SAFETY: the context `egl` made is current, so its loader returns its functions.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.api
                    .get_proc_address(name)
                    .map_or(std::ptr::null(), |f| f as *const c_void)
            })
        };
        // SAFETY: the context is current, and GL_VERSION is always a valid query.
        log::info!("OpenGL: {}", unsafe {
            gl.get_parameter_string(glow::VERSION)
        });

        // SAFETY: the context is current; everything used is created in this block.
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
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_color").as_ref(), 1);
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_image").as_ref(), 2);
            let max_texture_size = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0) as u32;

            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = (VERTEX_FLOATS * 4) as i32;
            for (index, size, offset) in [(0, 2, 0), (1, 2, 2), (2, 4, 4), (3, 1, 8)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset * 4);
            }

            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let texture = create_texture(&gl, glow::TEXTURE0, glow::R8, glow::RED, ATLAS_SIZE)?;
            let color_texture = create_texture(
                &gl,
                glow::TEXTURE1,
                glow::RGBA8,
                glow::RGBA,
                COLOR_ATLAS_SIZE,
            )?;

            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);

            Ok(Renderer {
                gl,
                program,
                vao,
                vbo,
                viewport_uniform,
                texture,
                color_texture,
                images: HashMap::new(),
                max_texture_size,
                oversized_images: HashSet::new(),
                atlas: Atlas::new(ATLAS_SIZE, COLOR_ATLAS_SIZE),
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

    /// Replaces the fonts, after a config reload.
    pub fn set_fonts(&mut self, fonts: Fonts) {
        self.fonts = fonts;
        self.atlas.clear();
    }

    pub fn set_font_size(&mut self, px: f32) {
        self.fonts.set_size(px);
        self.atlas.clear();
    }

    /// Draws and presents a frame (this commits the surface). Returns
    /// whether another frame is needed because the glyph atlas overflowed.
    pub fn draw(&mut self, frame: &Frame) -> Result<bool, String> {
        let metrics = self.fonts.metrics;
        let theme = self.theme.effective(frame.screen);
        let gl = &self.gl;
        let (texture, color_texture) = (self.texture, self.color_texture);
        // SAFETY: the context is current; `store` gets positions inside the atlas textures.
        let upload = |x: u32, y: u32, glyph: &Glyph| unsafe {
            let (unit, texture, format) = if glyph.color {
                (glow::TEXTURE1, color_texture, glow::RGBA)
            } else {
                (glow::TEXTURE0, texture, glow::RED)
            };
            gl.active_texture(unit);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                glyph.width as i32,
                glyph.height as i32,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&glyph.data)),
            );
        };
        let batch: Batch = super::build(
            frame,
            &theme,
            metrics,
            &mut self.atlas.cache(&mut self.fonts, upload),
        );

        // Upload new images, and free those the program deleted.
        let screen = frame.screen;
        self.images.retain(|uid, texture| {
            let live = screen.image(*uid).is_some();
            if !live {
                // SAFETY: the context is current, and this texture isn't used after this.
                unsafe { gl.delete_texture(*texture) };
            }
            live
        });
        for &(_, uid) in &batch.images {
            if self.images.contains_key(&uid) {
                continue;
            }
            let Some(image) = screen.image(uid) else {
                continue;
            };
            if image.width.max(image.height) > self.max_texture_size {
                // Drawn every frame; once is enough to say so.
                if !self.oversized_images.insert(uid) {
                    continue;
                }
                log::warn!(
                    "image of {}x{} exceeds the texture size limit",
                    image.width,
                    image.height
                );
                continue;
            }
            // SAFETY: the context is current, and `image` is checked against the size limit.
            match unsafe { upload_image(gl, image) } {
                Ok(texture) => {
                    self.images.insert(uid, texture);
                }
                Err(e) => log::warn!("cannot upload image: {e}"),
            }
        }

        let (w, h) = self.size;
        let [r, g, b] = theme.background.to_f32();
        let a = theme.opacity;
        // SAFETY: the context is current; every object used was made by `new` and lives as long as
        // the renderer, and the vertex slice outlives the upload.
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
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color_texture));
            // Image quads need their own texture; draw the runs between them.
            let draw = |first: usize, end: usize| {
                if end > first {
                    gl.draw_arrays(glow::TRIANGLES, first as i32, (end - first) as i32);
                }
            };
            let mut start = 0;
            gl.active_texture(glow::TEXTURE2);
            for &(first, uid) in &batch.images {
                draw(start, first);
                if let Some(&texture) = self.images.get(&uid) {
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    draw(first, first + 6);
                }
                start = first + 6;
            }
            draw(start, batch.vertices.len() / VERTEX_FLOATS);
        }
        self.egl
            .api
            .swap_buffers(self.egl.display, self.egl.surface)
            .map_err(|e| format!("eglSwapBuffers: {e}"))?;
        Ok(self.atlas.take_overflowed())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: the context is current, and the quads drawn were uploaded above.
        unsafe {
            self.gl.delete_texture(self.texture);
            self.gl.delete_texture(self.color_texture);
            for &texture in self.images.values() {
                self.gl.delete_texture(texture);
            }
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
        }
    }
}
