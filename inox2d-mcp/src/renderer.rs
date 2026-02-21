use std::ptr;

use glow::HasContext;
use image::{ImageBuffer, Rgba};

use inox2d::model::Model;
use inox2d::render::InoxRendererExt;
use inox2d_opengl::OpenglRenderer;

use glam::Vec2;

/// Raw CGL context that must be kept alive for GL operations.
pub struct GlState {
	cgl_context: cgl::CGLContextObj,
	cgl_pixel_format: cgl::CGLPixelFormatObj,
}

impl Drop for GlState {
	fn drop(&mut self) {
		unsafe {
			cgl::CGLSetCurrentContext(ptr::null_mut());
			cgl::CGLDestroyContext(self.cgl_context);
			cgl::CGLDestroyPixelFormat(self.cgl_pixel_format);
		}
	}
}

/// Headless OpenGL renderer for inox2d puppets.
pub struct HeadlessRenderer {
	pub gl_renderer: OpenglRenderer,
	/// Separate glow context for FBO operations (shares GL state with renderer's context).
	gl: glow::Context,
	pub fbo: glow::Framebuffer,
	pub fbo_texture: glow::Texture,
	pub fbo_depth_stencil: glow::Renderbuffer,
	pub width: u32,
	pub height: u32,
	_gl_state: GlState,
}

/// Load a GL function pointer from OpenGL.framework via dlsym.
fn load_gl_symbol(symbol: &str) -> *const std::ffi::c_void {
	use std::ffi::CString;
	extern "C" {
		fn dlopen(filename: *const i8, flags: i32) -> *mut std::ffi::c_void;
		fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
	}
	const RTLD_LAZY: i32 = 0x1;

	struct SendPtr(*mut std::ffi::c_void);
	unsafe impl Send for SendPtr {}
	unsafe impl Sync for SendPtr {}

	static OPENGL_LIB: std::sync::OnceLock<SendPtr> = std::sync::OnceLock::new();
	let handle = OPENGL_LIB.get_or_init(|| unsafe {
		let path =
			CString::new("/System/Library/Frameworks/OpenGL.framework/Versions/Current/OpenGL")
				.unwrap();
		SendPtr(dlopen(path.as_ptr(), RTLD_LAZY))
	}).0;

	if handle.is_null() {
		return ptr::null();
	}

	let name = CString::new(symbol).unwrap();
	unsafe { dlsym(handle, name.as_ptr()) as *const std::ffi::c_void }
}

/// Create a headless OpenGL context on macOS using CGL directly.
///
/// CGL allows `CGLSetCurrentContext` without a drawable, which is
/// sufficient for FBO-based offscreen rendering.
///
/// Returns two glow contexts (one for the renderer, one for FBO ops) and the GL state.
pub fn create_headless_gl_context() -> Result<(glow::Context, glow::Context, GlState), String> {
	unsafe {
		// kCGLOGLPVersion_3_2_Core = 0x3200 (not exported by the cgl crate)
		const KCG_LOGLP_VERSION_3_2_CORE: cgl::CGLPixelFormatAttribute = 0x3200;

		let attribs: [cgl::CGLPixelFormatAttribute; 9] = [
			cgl::kCGLPFAOpenGLProfile,
			KCG_LOGLP_VERSION_3_2_CORE,
			cgl::kCGLPFAColorSize,
			24,
			cgl::kCGLPFAAlphaSize,
			8,
			cgl::kCGLPFADepthSize,
			24,
			0, // terminator
		];

		let mut pixel_format: cgl::CGLPixelFormatObj = ptr::null_mut();
		let mut num_formats: cgl::GLint = 0;

		let err =
			cgl::CGLChoosePixelFormat(attribs.as_ptr(), &mut pixel_format, &mut num_formats);
		if err != cgl::kCGLNoError || pixel_format.is_null() {
			return Err(format!("CGLChoosePixelFormat failed: error {err}"));
		}

		let mut cgl_context: cgl::CGLContextObj = ptr::null_mut();
		let err = cgl::CGLCreateContext(pixel_format, ptr::null_mut(), &mut cgl_context);
		if err != cgl::kCGLNoError || cgl_context.is_null() {
			cgl::CGLDestroyPixelFormat(pixel_format);
			return Err(format!("CGLCreateContext failed: error {err}"));
		}

		let err = cgl::CGLSetCurrentContext(cgl_context);
		if err != cgl::kCGLNoError {
			cgl::CGLDestroyContext(cgl_context);
			cgl::CGLDestroyPixelFormat(pixel_format);
			return Err(format!("CGLSetCurrentContext failed: error {err}"));
		}

		let gl_for_renderer =
			glow::Context::from_loader_function(|s| load_gl_symbol(s) as *const _);
		let gl_for_fbo =
			glow::Context::from_loader_function(|s| load_gl_symbol(s) as *const _);

		let state = GlState {
			cgl_context,
			cgl_pixel_format: pixel_format,
		};

		Ok((gl_for_renderer, gl_for_fbo, state))
	}
}

impl HeadlessRenderer {
	/// Create a new headless renderer for the given model.
	///
	/// `gl_for_renderer` is consumed by `OpenglRenderer::new()`.
	/// `gl_for_fbo` is kept for FBO operations.
	pub fn new(
		gl_for_renderer: glow::Context,
		gl_for_fbo: glow::Context,
		gl_state: GlState,
		model: &Model,
		width: u32,
		height: u32,
	) -> Result<Self, String> {
		let mut gl_renderer = OpenglRenderer::new(gl_for_renderer, model)
			.map_err(|e| format!("Failed to create OpenGL renderer: {e}"))?;

		// Create FBO for offscreen rendering
		let (fbo, fbo_texture, fbo_depth_stencil) = unsafe {
			let gl = &gl_for_fbo;
			let fbo = gl
				.create_framebuffer()
				.map_err(|e| format!("Failed to create FBO: {e}"))?;
			let tex = gl
				.create_texture()
				.map_err(|e| format!("Failed to create texture: {e}"))?;
			let rbo = gl
				.create_renderbuffer()
				.map_err(|e| format!("Failed to create RBO: {e}"))?;

			// Setup color attachment
			gl.bind_texture(glow::TEXTURE_2D, Some(tex));
			gl.tex_image_2d(
				glow::TEXTURE_2D,
				0,
				glow::RGBA8 as i32,
				width as i32,
				height as i32,
				0,
				glow::RGBA,
				glow::UNSIGNED_BYTE,
				None,
			);
			gl.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_MIN_FILTER,
				glow::LINEAR as i32,
			);
			gl.tex_parameter_i32(
				glow::TEXTURE_2D,
				glow::TEXTURE_MAG_FILTER,
				glow::LINEAR as i32,
			);

			// Setup depth/stencil attachment
			gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rbo));
			gl.renderbuffer_storage(
				glow::RENDERBUFFER,
				glow::DEPTH24_STENCIL8,
				width as i32,
				height as i32,
			);

			// Attach to FBO
			gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
			gl.framebuffer_texture_2d(
				glow::FRAMEBUFFER,
				glow::COLOR_ATTACHMENT0,
				glow::TEXTURE_2D,
				Some(tex),
				0,
			);
			gl.framebuffer_renderbuffer(
				glow::FRAMEBUFFER,
				glow::DEPTH_STENCIL_ATTACHMENT,
				glow::RENDERBUFFER,
				Some(rbo),
			);

			let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
			if status != glow::FRAMEBUFFER_COMPLETE {
				return Err(format!("Framebuffer incomplete: status {status}"));
			}

			gl.bind_framebuffer(glow::FRAMEBUFFER, None);

			(fbo, tex, rbo)
		};

		gl_renderer.resize(width, height);
		gl_renderer.camera.scale = Vec2::splat(0.15);

		// Direct the renderer's composite output to our FBO instead of framebuffer 0
		gl_renderer.set_target_framebuffer(Some(fbo));

		Ok(Self {
			gl_renderer,
			gl: gl_for_fbo,
			fbo,
			fbo_texture,
			fbo_depth_stencil,
			width,
			height,
			_gl_state: gl_state,
		})
	}

	/// Resize the FBO and renderer viewport.
	pub fn resize(&mut self, width: u32, height: u32) {
		self.width = width;
		self.height = height;

		unsafe {
			let gl = &self.gl;

			gl.bind_texture(glow::TEXTURE_2D, Some(self.fbo_texture));
			gl.tex_image_2d(
				glow::TEXTURE_2D,
				0,
				glow::RGBA8 as i32,
				width as i32,
				height as i32,
				0,
				glow::RGBA,
				glow::UNSIGNED_BYTE,
				None,
			);

			gl.bind_renderbuffer(glow::RENDERBUFFER, Some(self.fbo_depth_stencil));
			gl.renderbuffer_storage(
				glow::RENDERBUFFER,
				glow::DEPTH24_STENCIL8,
				width as i32,
				height as i32,
			);
		}

		self.gl_renderer.resize(width, height);
	}

	/// Render the puppet to PNG bytes.
	///
	/// `dt` is the elapsed time for physics simulation.
	/// Pass `0.0` for the first frame.
	///
	/// `param_overrides` are applied between `begin_frame()` (which resets
	/// params to defaults) and `end_frame()` (which applies them to nodes).
	pub fn render_to_png(
		&mut self,
		puppet: &mut inox2d::puppet::Puppet,
		dt: f32,
		param_overrides: &std::collections::HashMap<String, Vec2>,
	) -> Result<Vec<u8>, String> {
		unsafe {
			self.gl
				.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
			self.gl
				.viewport(0, 0, self.width as i32, self.height as i32);
			self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
			self.gl.clear(
				glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT,
			);
		}

		puppet.begin_frame();

		// Apply user-set param overrides after begin_frame() reset
		if let Some(param_ctx) = puppet.param_ctx.as_mut() {
			for (name, val) in param_overrides {
				let _ = param_ctx.set(name, *val);
			}
		}

		puppet.end_frame(dt);

		self.gl_renderer.on_begin_draw(puppet);
		self.gl_renderer.draw(puppet);
		self.gl_renderer.on_end_draw(puppet);

		let pixel_count = (self.width * self.height) as usize;
		let mut pixels = vec![0u8; pixel_count * 4];

		unsafe {
			self.gl
				.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
			self.gl.read_pixels(
				0,
				0,
				self.width as i32,
				self.height as i32,
				glow::RGBA,
				glow::UNSIGNED_BYTE,
				glow::PixelPackData::Slice(&mut pixels),
			);
			self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
		}

		// OpenGL reads bottom-to-top, flip vertically
		let row_bytes = self.width as usize * 4;
		for y in 0..self.height as usize / 2 {
			let top = y * row_bytes;
			let bottom = (self.height as usize - 1 - y) * row_bytes;
			for x in 0..row_bytes {
				pixels.swap(top + x, bottom + x);
			}
		}

		let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
			ImageBuffer::from_raw(self.width, self.height, pixels)
				.ok_or_else(|| "Failed to create image buffer".to_string())?;

		let mut png_data = Vec::new();
		let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
		img.write_with_encoder(encoder)
			.map_err(|e| format!("Failed to encode PNG: {e}"))?;

		Ok(png_data)
	}
}
