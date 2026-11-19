# CAD Viewer 2.0

[Česky](README.md) | **English**

A fast viewer for 3D models and drawings on Windows and Linux.

## Features
- **3D models STEP, IGES, STL:** assembly tree, hiding parts, sections, transparency
- **Measuring:** distances, diameters, angles, wall thickness, copy results to Excel
- **Mass and center of gravity** by material, collision detection between parts
- **DXF and DWG drawings:** layers, layouts, rulers, measuring
- **File thumbnails** in Windows Explorer and Linux file managers
- 3Dconnexion 3D mouse support

## New in 2.0
- full rewrite in Rust with GPU rendering (`wgpu`), no Python or Qt runtime
- 71% smaller install: 16 MB Windows installer (`CadProhlizec_setup_2.0.0.exe`), 30.6 MB Linux AppImage
- portable Windows build (`StepViewer-portable-2.0.0.zip`)
- faster loading and smooth rendering of large models and dense DXF/DWG drawings

## Download
From [Releases](../../releases/latest):
- **Windows:** `CadProhlizec_setup_2.0.0.exe` or portable `StepViewer-portable-2.0.0.zip`
- **Linux:** `cad-viewer-2.0.0-x86_64.AppImage` or `cad-viewer-2.0.0-linux-x86_64.tar.gz`

The user interface is in Czech. License GPL-3.0.
