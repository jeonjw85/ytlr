import { invoke } from "@tauri-apps/api/core";
let target = "local";
export function scopedApi() {
  const expectedTarget = target;
  return <T>(path: string, method = "GET", body?: unknown): Promise<T> =>
    invoke<T>("api", { path, method, body: body ?? null, expectedTarget });
}

export async function api<T>(
  path: string,
  method = "GET",
  body?: unknown,
): Promise<T> {
  return invoke<T>("api", {
    path,
    method,
    body: body ?? null,
    expectedTarget: target,
  });
}
export const openJob = (id: string, index?: number) =>
  invoke("open_job", { id, index: index ?? null });
export const listRemotes = () =>
  invoke<import("./types").Remote[]>("list_remotes");
export const saveRemote = (remote: import("./types").Remote) =>
  invoke<import("./types").Remote[]>("save_remote", { remote });
export const deleteRemote = (name: string) =>
  invoke<import("./types").Remote[]>("delete_remote", { name });
export const connectRemote = (name: string | null) => {
  target = name ?? "local";
  return invoke<string>("connect_remote", { name });
};
