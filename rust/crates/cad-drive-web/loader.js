// Starts the WebAssembly module. The module runs its start function on load, reads the
// query string, and either redirects to cadviewer://open?fileId=... or shows an error.
// Generated next to this file by wasm-bindgen: pkg/cad_drive_web.js and pkg/cad_drive_web_bg.wasm.
import init from "./pkg/cad_drive_web.js";

init().catch(() => {
  const status = document.getElementById("status");
  if (status) {
    status.textContent = "Stránku se nepodařilo spustit. Povolte JavaScript a WebAssembly, nebo zkuste jiný prohlížeč.";
  }
});
