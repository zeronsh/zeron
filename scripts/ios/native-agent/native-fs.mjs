import { InMemoryFs } from "just-bash/browser";

// Only metadata lives here. All workspace bytes and mutations belong to native.
export class NativeWorkspaceFs {
  constructor(request) { this.request = request; this.paths = ["/"]; }
  async call(method, path, args = {}) {
    const result = await this.request({ method, path, ...args });
    if (result.paths) this.paths = result.paths;
    return result.value;
  }
  async refresh() { this.paths = await this.call("index", "/"); }
  getAllPaths() { return [...this.paths]; }
  resolvePath(base, path) {
    const parts = (path.startsWith("/") ? path : `${base}/${path}`).split("/");
    const stack = [];
    for (const part of parts) {
      if (part === "..") stack.pop();
      else if (part && part !== ".") stack.push(part);
    }
    return "/" + stack.join("/");
  }
  // Reuse the library's encoding semantics without caching workspace files.
  async readFile(path, options) {
    const codec = new InMemoryFs();
    await codec.writeFile("/buffer", await this.call("readFile", path), "base64");
    return codec.readFile("/buffer", options);
  }
  async readFileBuffer(path) {
    const codec = new InMemoryFs();
    await codec.writeFile("/buffer", await this.call("readFile", path), "base64");
    return codec.readFileBuffer("/buffer");
  }
  async encoded(content, options) {
    const codec = new InMemoryFs();
    await codec.writeFile("/buffer", content, options);
    return codec.readFile("/buffer", "base64");
  }
  async writeFile(path, content, options) { await this.call("writeFile", path, { content: await this.encoded(content, options) }); }
  async appendFile(path, content, options) { await this.call("appendFile", path, { content: await this.encoded(content, options) }); }
  exists(path) { return this.call("exists", path); }
  async stat(path) { const stat = await this.call("stat", path); return { ...stat, mtime: new Date(stat.mtime) }; }
  lstat(path) { return this.stat(path); }
  async realpath(path) { await this.stat(path); return this.resolvePath("/", path); }
  mkdir(path, options) { return this.call("mkdir", path, { recursive: options?.recursive === true }); }
  readdir(path) { return this.call("readdir", path); }
  rm(path, options) { return this.call("rm", path, { recursive: options?.recursive === true, force: options?.force === true }); }
  mv(path, destination) { return this.call("mv", path, { destination }); }
  async cp(source, destination, options) {
    if (source === destination || destination.startsWith(source + "/")) throw new Error("Cannot copy a path into itself");
    const stat = await this.stat(source);
    if (stat.isDirectory) {
      if (!options?.recursive) throw new Error("Copying a directory requires recursive mode");
      await this.mkdir(destination, { recursive: true });
      for (const name of await this.readdir(source)) await this.cp(this.resolvePath(source, name), this.resolvePath(destination, name), options);
    } else await this.writeFile(destination, await this.readFileBuffer(source));
    await this.chmod(destination, stat.mode);
  }
  chmod(path, mode) { return this.call("chmod", path, { mode }); }
  utimes(path, _atime, mtime) { return this.call("utimes", path, { mtime: mtime.getTime() }); }
  async symlink() { throw new Error("Symbolic links are not supported"); }
  async link() { throw new Error("Hard links are not supported"); }
  async readlink() { throw new Error("Symbolic links are not supported"); }
}
