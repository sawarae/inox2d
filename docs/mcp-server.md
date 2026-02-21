# inox2d MCP Server

An MCP (Model Context Protocol) server that provides headless puppet loading, parameter manipulation, and PNG rendering for Inochi2D puppets.

## Building

```bash
cargo build -p inox2d-mcp --release
```

The binary is output to `target/release/inox2d-mcp`.

## Usage with Claude Code

Add to your Claude Code MCP settings (`~/.claude/claude_desktop_config.json` or project-level `.mcp.json`):

```json
{
  "mcpServers": {
    "inox2d": {
      "command": "/path/to/inox2d-mcp"
    }
  }
}
```

## Tools

### `load_puppet`

Load an Inochi2D puppet from an `.inp` file. Must be called before other tools.

| Parameter | Type   | Required | Description                          |
|-----------|--------|----------|--------------------------------------|
| `path`    | string | yes      | Path to the `.inp` file              |
| `width`   | u32    | no       | Render width in pixels (default 800) |
| `height`  | u32    | no       | Render height in pixels (default 800)|

Returns puppet name, parameter count, and texture count.

### `get_puppet_info`

Returns metadata about the loaded puppet: name, version, rigger, artist, copyright, license, contact, etc.

No parameters.

### `list_params`

Lists all puppet parameters with their properties.

No parameters.

Each entry contains:

| Field       | Description                        |
|-------------|------------------------------------|
| `name`      | Parameter name                     |
| `is_vec2`   | Whether the parameter uses both axes |
| `min_x/y`   | Minimum values                     |
| `max_x/y`   | Maximum values                     |
| `default_x/y` | Default values                   |

### `set_param`

Set a parameter value by name.

| Parameter | Type   | Required | Description                              |
|-----------|--------|----------|------------------------------------------|
| `name`    | string | yes      | Parameter name (from `list_params`)      |
| `x`       | f32    | yes      | X value                                  |
| `y`       | f32    | no       | Y value (only for Vec2 params, default 0)|

### `render`

Render the current puppet state to PNG.

| Parameter     | Type   | Required | Description                                          |
|---------------|--------|----------|------------------------------------------------------|
| `width`       | u32    | no       | Override render width                                |
| `height`      | u32    | no       | Override render height                               |
| `output_path` | string | no       | File path to save PNG. Omit for base64 image content.|

When `output_path` is omitted, returns the PNG as base64-encoded image content (viewable by MCP clients that support images). When provided, saves the file and returns the path and byte count.

## Example workflow

A typical session from an MCP client:

1. **Load** a puppet file:
   ```
   load_puppet { "path": "/path/to/puppet.inp" }
   ```

2. **Inspect** the puppet:
   ```
   get_puppet_info {}
   list_params {}
   ```

3. **Manipulate** parameters:
   ```
   set_param { "name": "Eye L Open", "x": 0.5 }
   set_param { "name": "Mouth Open", "x": 1.0 }
   ```

4. **Render** to a file:
   ```
   render { "output_path": "/tmp/puppet.png" }
   ```

   Or get base64 image content directly:
   ```
   render {}
   ```

## Requirements

- macOS (uses CGL for headless OpenGL context)
- An `.inp` puppet file (Inochi2D puppet format)

## Architecture

The server runs over stdio using the `rmcp` crate. Rendering uses a headless OpenGL context via glutin (CGL pbuffer surface) with an offscreen FBO. The `inox2d` and `inox2d-opengl` crates handle puppet parsing and GPU rendering.
