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

Load an Inochi2D puppet from an `.inp` or `.inx` file. Must be called before other tools.

| Parameter | Type   | Required | Description                          |
|-----------|--------|----------|--------------------------------------|
| `path`    | string | yes      | Path to the `.inp` or `.inx` file    |
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
| `dt`          | f32    | no       | Physics simulation time in seconds (default 0). See below.|

When `output_path` is omitted, returns the PNG as base64-encoded image content (viewable by MCP clients that support images). When provided, saves the file and returns the path and byte count.

Camera scale adjusts automatically with resolution so the puppet maintains the same apparent size at any viewport size.

## Example workflow

A typical session from an MCP client:

1. **Load** a puppet file:
   ```
   load_puppet { "path": "/path/to/puppet.inx" }
   ```

2. **Inspect** the puppet:
   ```
   get_puppet_info {}
   list_params {}
   ```

3. **Manipulate** parameters:
   ```
   set_param { "name": "Eye:: Left:: Blink", "x": 0.5 }
   set_param { "name": "Head:: Yaw-Pitch", "x": 1.0, "y": 0.0 }
   ```

4. **Render** to a file:
   ```
   render { "output_path": "/tmp/puppet.png" }
   ```

   Or get base64 image content directly:
   ```
   render {}
   ```

## Rendering examples

### Head turning

Set `Head:: Yaw-Pitch` x value (-1.0 = left, 1.0 = right):

```
load_puppet { "path": "puppet.inx", "width": 800, "height": 800 }
set_param   { "name": "Head:: Yaw-Pitch", "x": 1.0, "y": 0.0 }
render      { "output_path": "/tmp/head_right.png" }

set_param   { "name": "Head:: Yaw-Pitch", "x": -1.0, "y": 0.0 }
render      { "output_path": "/tmp/head_left.png" }
```

### Eye blink

Set both eye blink params to 1.0 (closed) or 0.0 (open):

```
set_param { "name": "Eye:: Left:: Blink", "x": 1.0 }
set_param { "name": "Eye:: Right:: Blink", "x": 1.0 }
render    { "output_path": "/tmp/blink.png" }
```

### Physics simulation (hair/clothes sway)

Use the `dt` parameter on `render` to run physics for a given duration.
Combine with body/head tilt for visible motion:

```
set_param { "name": "Body:: Yaw-Pitch", "x": 0.8, "y": 0.0 }
set_param { "name": "Head:: Yaw-Pitch", "x": 0.5, "y": -0.3 }
render    { "output_path": "/tmp/physics.png", "dt": 0.5 }
```

The renderer steps physics at 60 fps intervals for the given duration,
then captures the final frame. This produces natural hair and clothing sway.

### High-resolution rendering

Pass `width`/`height` to `render` (camera scale adjusts automatically):

```
render { "width": 1600, "height": 1600, "output_path": "/tmp/hires.png" }
```

## Requirements

- macOS (uses CGL for headless OpenGL context)
- An `.inp` or `.inx` puppet file (Inochi2D puppet format)

## Architecture

The server runs over stdio using the `rmcp` crate. Rendering uses a headless OpenGL context via CGL (no window/drawable needed) with an offscreen FBO. The `inox2d` and `inox2d-opengl` crates handle puppet parsing and GPU rendering.
