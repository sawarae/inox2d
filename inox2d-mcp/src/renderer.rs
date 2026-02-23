use std::ptr;

use glow::HasContext;
use image::{ImageBuffer, Rgba};

use inox2d::model::Model;
use inox2d::render::InoxRendererExt;
use inox2d_opengl::OpenglRenderer;

use glam::Vec2;

// ─── macOS (CGL) ──────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
pub struct GlState {
	cgl_context: cgl::CGLContextObj,
	cgl_pixel_format: cgl::CGLPixelFormatObj,
}

#[cfg(target_os = "macos")]
impl Drop for GlState {
	fn drop(&mut self) {
		unsafe {
			cgl::CGLSetCurrentContext(ptr::null_mut());
			cgl::CGLDestroyContext(self.cgl_context);
			cgl::CGLDestroyPixelFormat(self.cgl_pixel_format);
		}
	}
}

/// Load a GL function pointer from OpenGL.framework via dlsym.
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

// ─── Linux (EGL) ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub struct GlState {
	egl: khronos_egl::DynamicInstance<khronos_egl::EGL1_4>,
	display: khronos_egl::Display,
	context: khronos_egl::Context,
	surface: khronos_egl::Surface,
}

#[cfg(target_os = "linux")]
impl Drop for GlState {
	fn drop(&mut self) {
		let _ = self.egl.make_current(self.display, None, None, None);
		let _ = self.egl.destroy_surface(self.display, self.surface);
		let _ = self.egl.destroy_context(self.display, self.context);
		let _ = self.egl.terminate(self.display);
	}
}

/// Load an OpenGL symbol via eglGetProcAddress, with dlsym fallback.
#[cfg(target_os = "linux")]
fn load_gl_symbol_egl(
	egl: &khronos_egl::DynamicInstance<khronos_egl::EGL1_4>,
	symbol: &str,
) -> *const std::ffi::c_void {
	// Try eglGetProcAddress first
	if let Some(f) = egl.get_proc_address(symbol) {
		return f as *const std::ffi::c_void;
	}

	// Fall back to dlsym on libGL.so.1
	use std::ffi::CString;
	extern "C" {
		fn dlopen(filename: *const i8, flags: i32) -> *mut std::ffi::c_void;
		fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
	}
	const RTLD_LAZY: i32 = 0x1;
	const RTLD_GLOBAL: i32 = 0x100;

	struct SendPtr(*mut std::ffi::c_void);
	unsafe impl Send for SendPtr {}
	unsafe impl Sync for SendPtr {}

	static LIBGL: std::sync::OnceLock<SendPtr> = std::sync::OnceLock::new();
	let handle = LIBGL.get_or_init(|| unsafe {
		// Try libGL.so.1 first, then libGL.so
		let path1 = CString::new("libGL.so.1").unwrap();
		let h = dlopen(path1.as_ptr(), RTLD_LAZY | RTLD_GLOBAL);
		if !h.is_null() {
			return SendPtr(h);
		}
		let path2 = CString::new("libGL.so").unwrap();
		SendPtr(dlopen(path2.as_ptr(), RTLD_LAZY | RTLD_GLOBAL))
	}).0;

	if handle.is_null() {
		return ptr::null();
	}

	let name = CString::new(symbol).unwrap();
	unsafe { dlsym(handle, name.as_ptr()) as *const std::ffi::c_void }
}

/// Create a headless OpenGL context on Linux using EGL with a pbuffer surface.
#[cfg(target_os = "linux")]
pub fn create_headless_gl_context() -> Result<(glow::Context, glow::Context, GlState), String> {
	use khronos_egl as egl;

	let instance = unsafe {
		egl::DynamicInstance::<egl::EGL1_4>::load_required()
			.map_err(|e| format!("Failed to load EGL: {e}"))?
	};

	let display = unsafe {
		instance
			.get_display(egl::DEFAULT_DISPLAY)
			.ok_or("No default EGL display available")?
	};

	instance
		.initialize(display)
		.map_err(|e| format!("EGL initialize failed: {e}"))?;

	instance
		.bind_api(egl::OPENGL_API)
		.map_err(|_| "Failed to bind OpenGL API (EGL_KHR_client_get_all_proc_addresses may be needed)")?;

	let config_attribs = [
		egl::RED_SIZE,
		8,
		egl::GREEN_SIZE,
		8,
		egl::BLUE_SIZE,
		8,
		egl::ALPHA_SIZE,
		8,
		egl::DEPTH_SIZE,
		24,
		egl::STENCIL_SIZE,
		8,
		egl::RENDERABLE_TYPE,
		egl::OPENGL_BIT,
		egl::SURFACE_TYPE,
		egl::PBUFFER_BIT,
		egl::NONE,
	];

	let config = instance
		.choose_first_config(display, &config_attribs)
		.map_err(|e| format!("EGL choose config failed: {e}"))?
		.ok_or("No suitable EGL config found (OpenGL + pbuffer)")?;

	let pbuffer_attribs = [egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE];

	let surface = instance
		.create_pbuffer_surface(display, config, &pbuffer_attribs)
		.map_err(|e| format!("EGL create pbuffer surface failed: {e}"))?;

	// EGL 1.5 / EGL_KHR_create_context attribute values
	const EGL_CONTEXT_MAJOR_VERSION: egl::Int = 0x3098;
	const EGL_CONTEXT_MINOR_VERSION: egl::Int = 0x30FB;

	let ctx_attribs = [
		EGL_CONTEXT_MAJOR_VERSION,
		3,
		EGL_CONTEXT_MINOR_VERSION,
		2,
		egl::NONE,
	];

	let context = instance
		.create_context(display, config, None, &ctx_attribs)
		.map_err(|e| format!("EGL create context failed: {e}"))?;

	instance
		.make_current(display, Some(surface), Some(surface), Some(context))
		.map_err(|e| format!("EGL make current failed: {e}"))?;

	let gl_for_renderer = unsafe {
		glow::Context::from_loader_function(|s| load_gl_symbol_egl(&instance, s))
	};
	let gl_for_fbo = unsafe {
		glow::Context::from_loader_function(|s| load_gl_symbol_egl(&instance, s))
	};

	let state = GlState {
		egl: instance,
		display,
		context,
		surface,
	};

	Ok((gl_for_renderer, gl_for_fbo, state))
}

// ─── Unsupported platforms ────────────────────────────────────────────────────

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("inox2d-mcp only supports macOS and Linux");

// ─── Platform-agnostic HeadlessRenderer ──────────────────────────────────────

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
		gl_renderer.camera.scale = Self::scale_for_size(width, height);

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

	/// Camera scale proportional to viewport size.
	/// 0.15 at 800px is the reference; scale linearly with the smaller dimension.
	fn scale_for_size(width: u32, height: u32) -> Vec2 {
		let base = 800.0_f32;
		let base_scale = 0.15_f32;
		let factor = width.min(height) as f32 / base;
		Vec2::splat(base_scale * factor)
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
		self.gl_renderer.camera.scale = Self::scale_for_size(width, height);
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
