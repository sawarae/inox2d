use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;

use base64::Engine;
use glam::Vec2;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use serde::{Deserialize, Serialize};

use crate::renderer::{self, HeadlessRenderer};
use inox2d::formats::inp::parse_inp;
use inox2d::model::Model;

/// State shared across all MCP tool calls.
pub struct PuppetState {
	pub model: Model,
	pub renderer: HeadlessRenderer,
	/// User-set param overrides. Applied between begin_frame() and end_frame()
	/// since begin_frame() resets all params to defaults.
	pub param_overrides: HashMap<String, Vec2>,
}

// Safety: PuppetState contains GL resources with raw pointers, but we only access
// them through a Mutex which ensures exclusive access. GL calls are only made
// from within the mutex lock, on the same thread that created the context.
unsafe impl Send for PuppetState {}

/// The MCP server for inox2d puppet operations.
pub struct InoxMcpServer {
	tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
	state: Mutex<Option<PuppetState>>,
}

impl InoxMcpServer {
	pub fn new() -> Self {
		Self {
			tool_router: Self::tool_router(),
			state: Mutex::new(None),
		}
	}
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LoadPuppetParams {
	/// Path to the .inp file to load
	pub path: String,
	/// Render width in pixels (default: 800)
	pub width: Option<u32>,
	/// Render height in pixels (default: 800)
	pub height: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SetParamParams {
	/// Name of the parameter to set
	pub name: String,
	/// X value for the parameter
	pub x: f32,
	/// Y value for the parameter (only used for Vec2 params)
	pub y: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RenderParams {
	/// Width in pixels (uses current width if not specified)
	pub width: Option<u32>,
	/// Height in pixels (uses current height if not specified)
	pub height: Option<u32>,
	/// File path to save the PNG to. If not specified, returns base64-encoded image content.
	pub output_path: Option<String>,
}

fn text_result(text: impl Into<String>) -> CallToolResult {
	CallToolResult::success(vec![Content::text(text)])
}

fn error_result(text: impl Into<String>) -> CallToolResult {
	CallToolResult::error(vec![Content::text(text)])
}

#[tool_router]
impl InoxMcpServer {
	/// Load an Inochi2D puppet from an .inp file. Must be called before other puppet operations.
	#[tool(description = "Load an Inochi2D puppet from an .inp file. Must be called before other puppet operations.")]
	async fn load_puppet(
		&self,
		Parameters(params): Parameters<LoadPuppetParams>,
	) -> Result<CallToolResult, rmcp::ErrorData> {
		let width = params.width.unwrap_or(800);
		let height = params.height.unwrap_or(800);

		// Read and parse the puppet file
		let data = match fs::read(&params.path) {
			Ok(data) => data,
			Err(e) => {
				return Ok(error_result(format!(
					"Failed to read file '{}': {e}",
					params.path
				)))
			}
		};

		let mut model = match parse_inp(data.as_slice()) {
			Ok(model) => model,
			Err(e) => return Ok(error_result(format!("Failed to parse puppet: {e}"))),
		};

		// Initialize puppet subsystems
		model.puppet.init_transforms();
		model.puppet.init_rendering();
		model.puppet.init_params();
		model.puppet.init_physics();

		// Create headless GL context
		let (gl_for_renderer, gl_for_fbo, gl_state) = match renderer::create_headless_gl_context()
		{
			Ok(v) => v,
			Err(e) => return Ok(error_result(format!("Failed to create GL context: {e}"))),
		};

		// Create headless renderer
		let headless = match HeadlessRenderer::new(
			gl_for_renderer,
			gl_for_fbo,
			gl_state,
			&model,
			width,
			height,
		) {
			Ok(r) => r,
			Err(e) => return Ok(error_result(format!("Failed to create renderer: {e}"))),
		};

		let puppet_name = model
			.puppet
			.meta
			.name
			.clone()
			.unwrap_or_else(|| "<unnamed>".to_string());
		let param_count = model.puppet.params.len();
		let texture_count = model.textures.len();

		let mut state = self.state.lock().unwrap();
		*state = Some(PuppetState {
			model,
			renderer: headless,
			param_overrides: HashMap::new(),
		});

		let info = serde_json::json!({
			"name": puppet_name,
			"param_count": param_count,
			"texture_count": texture_count,
			"render_width": width,
			"render_height": height,
		});

		Ok(text_result(serde_json::to_string_pretty(&info).unwrap()))
	}

	/// Get metadata about the currently loaded puppet (name, version, rigger, artist, etc.)
	#[tool(description = "Get metadata about the currently loaded puppet (name, version, rigger, artist, etc.)")]
	async fn get_puppet_info(&self) -> Result<CallToolResult, rmcp::ErrorData> {
		let state = self.state.lock().unwrap();
		let state = match state.as_ref() {
			Some(s) => s,
			None => return Ok(error_result("No puppet loaded. Call load_puppet first.")),
		};

		let meta = &state.model.puppet.meta;
		let info = serde_json::json!({
			"name": meta.name,
			"version": meta.version,
			"rigger": meta.rigger,
			"artist": meta.artist,
			"copyright": meta.copyright,
			"license_url": meta.license_url,
			"contact": meta.contact,
			"reference": meta.reference,
			"thumbnail_id": meta.thumbnail_id,
			"preserve_pixels": meta.preserve_pixels,
		});

		Ok(text_result(serde_json::to_string_pretty(&info).unwrap()))
	}

	/// List all parameters of the loaded puppet with their names, ranges, defaults, and whether they are Vec2.
	#[tool(description = "List all parameters of the loaded puppet with their names, ranges, defaults, and whether they are Vec2.")]
	async fn list_params(&self) -> Result<CallToolResult, rmcp::ErrorData> {
		let state = self.state.lock().unwrap();
		let state = match state.as_ref() {
			Some(s) => s,
			None => return Ok(error_result("No puppet loaded. Call load_puppet first.")),
		};

		let params: Vec<serde_json::Value> = state
			.model
			.puppet
			.params
			.iter()
			.map(|(name, param)| {
				serde_json::json!({
					"name": name,
					"is_vec2": param.is_vec2,
					"min_x": param.min.x,
					"min_y": param.min.y,
					"max_x": param.max.x,
					"max_y": param.max.y,
					"default_x": param.defaults.x,
					"default_y": param.defaults.y,
				})
			})
			.collect();

		Ok(text_result(serde_json::to_string_pretty(&params).unwrap()))
	}

	/// Set a parameter value on the loaded puppet. Use list_params to see available parameters.
	#[tool(description = "Set a parameter value on the loaded puppet. Use list_params to see available parameters.")]
	async fn set_param(
		&self,
		Parameters(params): Parameters<SetParamParams>,
	) -> Result<CallToolResult, rmcp::ErrorData> {
		let mut state = self.state.lock().unwrap();
		let state = match state.as_mut() {
			Some(s) => s,
			None => return Ok(error_result("No puppet loaded. Call load_puppet first.")),
		};

		// Validate param name exists
		if !state.model.puppet.params.contains_key(&params.name) {
			return Ok(error_result(format!(
				"No parameter named '{}'",
				params.name
			)));
		}

		let val = Vec2::new(params.x, params.y.unwrap_or(0.0));
		state.param_overrides.insert(params.name.clone(), val);

		Ok(text_result(format!(
			"Parameter '{}' set to ({}, {})",
			params.name, val.x, val.y
		)))
	}

	/// Render the current puppet state to a PNG image. Returns base64-encoded PNG data or saves to a file path.
	#[tool(description = "Render the current puppet state to a PNG image. Returns base64-encoded PNG data or saves to a file path.")]
	async fn render(
		&self,
		Parameters(params): Parameters<RenderParams>,
	) -> Result<CallToolResult, rmcp::ErrorData> {
		let mut state = self.state.lock().unwrap();
		let state = match state.as_mut() {
			Some(s) => s,
			None => return Ok(error_result("No puppet loaded. Call load_puppet first.")),
		};

		// Resize if requested
		if let (Some(w), Some(h)) = (params.width, params.height) {
			state.renderer.resize(w, h);
		}

		// Render
		let png_data = match state
			.renderer
			.render_to_png(&mut state.model.puppet, 0.0, &state.param_overrides)
		{
			Ok(data) => data,
			Err(e) => return Ok(error_result(format!("Render failed: {e}"))),
		};

		if let Some(output_path) = params.output_path {
			// Save to file
			match fs::write(&output_path, &png_data) {
				Ok(()) => Ok(text_result(format!(
					"Rendered PNG saved to: {output_path} ({} bytes)",
					png_data.len()
				))),
				Err(e) => Ok(error_result(format!("Failed to write file: {e}"))),
			}
		} else {
			// Return as base64 image content
			let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
			Ok(CallToolResult::success(vec![Content::image(
				b64,
				"image/png",
			)]))
		}
	}
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for InoxMcpServer {
	fn get_info(&self) -> ServerInfo {
		ServerInfo {
			capabilities: ServerCapabilities::builder().enable_tools().build(),
			instructions: Some(
				"inox2d MCP server for puppet operation and PNG rendering. \
				 Load a puppet with load_puppet, inspect with get_puppet_info and list_params, \
				 manipulate with set_param, and render with render."
					.into(),
			),
			..ServerInfo::default()
		}
	}
}
