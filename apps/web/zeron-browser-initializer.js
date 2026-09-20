export default function initializer() {
  const loading = document.getElementById("loading");
  const label = loading.querySelector("span");
  const bar = loading.querySelector("progress");
  const megabytes = bytes => (bytes / 1048576).toFixed(0);

  return {
    onProgress({ current, total }) {
      if (current >= total) {
        // Downloaded; compiling and booting has no measurable progress.
        bar.removeAttribute("value");
        label.textContent = "Starting Zeron…";
        return;
      }
      bar.max = total;
      bar.value = current;
      label.textContent = `Loading Zeron… ${Math.floor((current / total) * 100)}% (${megabytes(current)} of ${megabytes(total)} MB)`;
    },
    // Trunk swallows the rejection after calling this, so window error events never fire for it.
    onFailure(error) {
      console.error(error);
      loading.classList.add("failed");
      label.textContent = "Couldn't load Zeron. Reload to try again.";
    },
  };
}
