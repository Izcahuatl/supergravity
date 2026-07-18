export function initSettings(_state, _refresh) {
  document.getElementById("open-settings").onclick = () =>
    document.getElementById("settings").classList.remove("hidden");
  document.getElementById("close-settings").onclick = () =>
    document.getElementById("settings").classList.add("hidden");
}
