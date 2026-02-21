use std::ffi::CString;
use std::num::NonZeroU32;

use glow::HasContext;
use image::{ImageBuffer, Rgba};

use inox2d::model::Model;
use inox2d::render::InoxRendererExt;
use inox2d_opengl::OpenglRenderer;

use glam::Vec2;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext, Version};
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{PbufferSurface, Surface, SurfaceAttributesBuilder};

use raw_window_handle::{AppKitDisplayHandle, RawDisplayHandle};

/// Holds the GL context and surface needed to keep them alive.
///
/// These fields are never directly read, but must be kept alive to
/// maintain the GL context. Dropping them would invalidate the context.
#[allow(dead_code)]
pub struct GlState {
	gl_context: PossiblyCurrentContext,
	gl_surface: Surface<PbufferSurface>,
	gl_display: Display,
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

/// Load a glow::Context from a glutin display.
fn load_gl(display: &Display) -> glow::Context {
	unsafe {
		glow::Context::from_loader_function(|symbol| {
			display.get_proc_address(&CString::new(symbol).unwrap()) as *const _
		})
	}
}

/// Create a headless OpenGL context on macOS using CGL via glutin.
///
/// Returns two glow contexts (one for the renderer, one for FBO ops) and the GL state.
pub fn create_headless_gl_context() -> Result<(glow::Context, glow::Context, GlState), String> {
	// On macOS, create a CGL display
	let display = unsafe {
		Display::new(
			RawDisplayHandle::AppKit(AppKitDisplayHandle::empty()),
			DisplayApiPreference::Cgl,
		)
	}
	.map_err(|e| format!("Failed to create display: {e}"))?;

	let template = ConfigTemplateBuilder::new().with_alpha_size(8).build();

	let config = unsafe { display.find_configs(template) }
		.map_err(|e| format!("Failed to find configs: {e}"))?
		.next()
		.ok_or_else(|| "No suitable GL config found".to_string())?;

	// Create context
	let context_attrs = ContextAttributesBuilder::new().build(None);
	let fallback_attrs = ContextAttributesBuilder::new()
		.with_context_api(ContextApi::OpenGl(Some(Version::new(2, 1))))
		.build(None);

	let not_current_ctx = unsafe {
		display
			.create_context(&config, &context_attrs)
			.or_else(|_| display.create_context(&config, &fallback_attrs))
	}
	.map_err(|e| format!("Failed to create context: {e}"))?;

	// Create a small pbuffer surface to make context current
	let surface_attrs = SurfaceAttributesBuilder::<PbufferSurface>::new()
		.build(NonZeroU32::new(1).unwrap(), NonZeroU32::new(1).unwrap());
	let surface = unsafe { display.create_pbuffer_surface(&config, &surface_attrs) }
		.map_err(|e| format!("Failed to create pbuffer surface: {e}"))?;

	// Make current
	let gl_context = not_current_ctx
		.make_current(&surface)
		.map_err(|e| format!("Failed to make context current: {e}"))?;

	// Load two separate glow::Context instances from the same display.
	// Both share the same underlying GL context state.
	let gl_for_renderer = load_gl(&display);
	let gl_for_fbo = load_gl(&display);

	let state = GlState {
		gl_context,
		gl_surface: surface,
		gl_display: display,
	};

	Ok((gl_for_renderer, gl_for_fbo, state))
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
			gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
			gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);

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

			// Resize color texture
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

			// Resize depth/stencil renderbuffer
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
	pub fn render_to_png(&mut self, puppet: &mut inox2d::puppet::Puppet, dt: f32) -> Result<Vec<u8>, String> {
		unsafe {
			// Bind our FBO
			self.gl
				.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
			self.gl
				.viewport(0, 0, self.width as i32, self.height as i32);

			// Clear
			self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
			self.gl.clear(
				glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT,
			);
		}

		// Run puppet frame
		puppet.begin_frame();
		puppet.end_frame(dt);

		// Draw
		self.gl_renderer.on_begin_draw(puppet);
		self.gl_renderer.draw(puppet);
		self.gl_renderer.on_end_draw(puppet);

		// Read pixels
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

		// Encode as PNG
		let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(self.width, self.height, pixels)
			.ok_or_else(|| "Failed to create image buffer".to_string())?;

		let mut png_data = Vec::new();
		let encoder = image::codecs::png::PngEncoder::new(&mut png_data);
		img.write_with_encoder(encoder)
			.map_err(|e| format!("Failed to encode PNG: {e}"))?;

		Ok(png_data)
	}
}
