// agent/protocol/src/files.rs: paths in the file store, in the one spelling
// the device uses - no leading or trailing `/`, `""` for the root.

const MAX_PATH = 4096;
const MAX_NAME = 255;

/** The names in `path`, checked; throws the device's words when it is not one. */
export function checkPath(path: string): string[] {
  if (path.length > MAX_PATH) {
    throw new Error(`${[...path].slice(0, 32).join("")}...: the path is too long`);
  }
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(path)) {
    throw new Error(`${JSON.stringify(path)}: control characters are not allowed`);
  }
  if (path.includes("\\")) {
    throw new Error(`${path}: use / to separate names, not \\`);
  }
  const trimmed = path.replace(/^\/+|\/+$/g, "");
  if (trimmed === "" || trimmed === ".") {
    return [];
  }
  const names: string[] = [];
  for (const name of trimmed.split("/")) {
    if (name === "") throw new Error(`${path}: empty name between slashes`);
    if (name === "." || name === "..") throw new Error(`${path}: . and .. are not allowed`);
    if (new TextEncoder().encode(name).length > MAX_NAME) {
      throw new Error(`${path}: a name is longer than ${MAX_NAME} bytes`);
    }
    names.push(name);
  }
  return names;
}

export function normalize(path: string): string {
  return checkPath(path).join("/");
}

/** `name` under `dir`, either of which may be the root. */
export function join(dir: string, name: string): string {
  if (dir === "") return name;
  if (name === "") return dir;
  return `${dir}/${name}`;
}

/** The directory `path` is in; the root's is the root. */
export function parent(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

/** The last name of `path`. */
export function base(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}
