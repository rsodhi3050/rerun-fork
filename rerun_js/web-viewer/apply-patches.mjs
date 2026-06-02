// One-shot script: apply the re_viewer.js + re_viewer.d.ts patches that
// build-wasm.mjs normally does after `cargo run -p re_dev_tools ...`.
//
// We call cargo separately (PATH inside Windows execSync inherits cmd.exe's
// PATH, which doesn't include ~/.cargo/bin), so build-wasm.mjs can't drive
// the full pipeline here. This script does ONLY the patching half.

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = path.resolve(fileURLToPath(import.meta.url));
const __dirname = path.dirname(__filename);

// === re_viewer.js: wrap in default-exported function + closure dtor guards ===
let code = fs.readFileSync(path.join(__dirname, "re_viewer.js"), "utf-8");

const wrap_start = `let wasm_bindgen = (function(exports) {`;
const wrap_end = `return Object.assign(__wbg_init, { initSync }, exports);
})({ __proto__: null });`;

if (code.indexOf(wrap_start) === -1) {
  throw new Error("re_viewer.js: missing wrap start marker — wasm-bindgen output drift?");
}
if (code.indexOf(wrap_end) === -1) {
  throw new Error("re_viewer.js: missing wrap end marker — wasm-bindgen output drift?");
}
// Idempotency guard: if we've already wrapped, do nothing.
if (code.startsWith("\nexport default function() {")) {
  console.log("re_viewer.js already wrapped, skipping module-wrap step");
} else {
  code = code.replace(wrap_start, "").replace(wrap_end, "");
  code = `
export default function() {
const exports = { __proto__: null };
${code}

function deinit() {
  __wbg_init.__wbindgen_wasm_module = null;
  wasmModule = null;
  wasm = null;
  cachedDataViewMemory0 = null;
  cachedFloat32ArrayMemory0 = null;
  cachedInt16ArrayMemory0 = null;
  cachedInt32ArrayMemory0 = null;
  cachedInt8ArrayMemory0 = null;
  cachedUint16ArrayMemory0 = null;
  cachedUint32ArrayMemory0 = null;
  cachedUint8ArrayMemory0 = null;
}

return Object.assign(__wbg_init, { initSync, deinit }, exports);
}
`;
  console.log("re_viewer.js: applied module-wrap");
}

// Guard CLOSURE_DTORS against null `wasm` during deinit.
const closure_dtors_original = `const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));`;

const closure_dtors_patch = `const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
        ? { register: () => {}, unregister: () => {} }
        : new FinalizationRegistry(state => {
        if (wasm) wasm.__wbindgen_destroy_closure(state.a, state.b);
    });`;

if (code.indexOf(closure_dtors_original) !== -1) {
  code = code.replace(closure_dtors_original, closure_dtors_patch);
  console.log("re_viewer.js: applied CLOSURE_DTORS patch");
} else if (code.indexOf(closure_dtors_patch) !== -1) {
  console.log("re_viewer.js: CLOSURE_DTORS patch already applied");
} else {
  throw new Error("re_viewer.js: missing CLOSURE_DTORS block — wasm-bindgen output drift?");
}

// Guard makeMutClosure against null `wasm` during deinit.
const make_mut_closure_original = `function makeMutClosure(arg0, arg1, f) {
        const state = { a: arg0, b: arg1, cnt: 1 };
        const real = (...args) => {

            // First up with a closure we increment the internal reference
            // count. This ensures that the Rust closure environment won't
            // be deallocated while we're invoking it.
            state.cnt++;
            const a = state.a;
            state.a = 0;
            try {
                return f(a, state.b, ...args);
            } finally {
                state.a = a;
                real._wbg_cb_unref();
            }
        };
        real._wbg_cb_unref = () => {
            if (--state.cnt === 0) {
                wasm.__wbindgen_destroy_closure(state.a, state.b);
                state.a = 0;
                CLOSURE_DTORS.unregister(state);
            }
        };
        CLOSURE_DTORS.register(real, state, state);
        return real;
    }`;

const make_mut_closure_patch = `function makeMutClosure(arg0, arg1, f) {
        const state = { a: arg0, b: arg1, cnt: 1 };
        const real = (...args) => {
            state.cnt++;
            const a = state.a;
            state.a = 0;
            try {
                if (!wasm) return;
                return f(a, state.b, ...args);
            } finally {
                state.a = a;
                real._wbg_cb_unref();
            }
        };
        real._wbg_cb_unref = () => {
            if (--state.cnt === 0) {
                if (wasm) wasm.__wbindgen_destroy_closure(state.a, state.b);
                state.a = 0;
                CLOSURE_DTORS.unregister(state);
            }
        };
        CLOSURE_DTORS.register(real, state, state);
        return real;
    }`;

if (code.indexOf(make_mut_closure_original) !== -1) {
  code = code.replace(make_mut_closure_original, make_mut_closure_patch);
  console.log("re_viewer.js: applied makeMutClosure patch");
} else if (code.indexOf(make_mut_closure_patch) !== -1) {
  console.log("re_viewer.js: makeMutClosure patch already applied");
} else {
  throw new Error("re_viewer.js: missing makeMutClosure block — wasm-bindgen output drift?");
}

fs.writeFileSync(path.join(__dirname, "re_viewer.js"), code);

// === re_viewer.d.ts: add WebHandle re-export + default export ===
let dts = fs.readFileSync(path.join(__dirname, "re_viewer.d.ts"), "utf-8");
const dts_tail = `
export type WebHandle = wasm_bindgen.WebHandle;
export default function(): wasm_bindgen;
`;
if (!dts.includes("export type WebHandle = wasm_bindgen.WebHandle;")) {
  dts = dts + dts_tail;
  fs.writeFileSync(path.join(__dirname, "re_viewer.d.ts"), dts);
  console.log("re_viewer.d.ts: appended WebHandle + default export");
} else {
  console.log("re_viewer.d.ts: WebHandle/default already present");
}
console.log("done.");
