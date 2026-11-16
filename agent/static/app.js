const $ = (id) => document.getElementById(id);
let state = { tickets: [] };
let selected = null;

async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { "content-type": "application/json", ...(options.headers || {}) } });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error?.message || "Request failed");
  return value;
}

function notify(message) { $("message").textContent = message; $("message").style.display = "block"; setTimeout(() => $("message").style.display = "none", 3000); }

function render() {
  $("current-project").textContent = state.projectRoot || "尚未选择";
  $("tickets").innerHTML = state.tickets.length ? state.tickets.map((ticket) => `<button class="ticket" data-id="${ticket.id}">${escapeHtml(ticket.title)}<small>${ticket.state} · ${ticket.id.slice(0, 18)}</small></button>`).join("") : '<p class="muted">暂无任务</p>';
  document.querySelectorAll(".ticket").forEach((button) => button.onclick = () => { selected = state.tickets.find((item) => item.id === button.dataset.id); renderDetail(); });
  if (selected) selected = state.tickets.find((item) => item.id === selected.id) || null;
  renderDetail();
}

function escapeHtml(value) { const node = document.createElement("span"); node.textContent = value; return node.innerHTML; }

function renderDetail() {
  $("detail").textContent = selected ? JSON.stringify(selected, null, 2) : "选择一个 Ticket";
  if (!selected) return $("actions").replaceChildren();
  const actions = $("actions"); actions.replaceChildren();
  const add = (label, fn, cls = "secondary") => { const button = document.createElement("button"); button.textContent = label; button.className = cls; button.onclick = fn; actions.append(button); };
  add("调用本地模型", () => act(`/api/tickets/${selected.id}/plan`, {}));
  add("导出审计", () => act(`/api/tickets/${selected.id}/audit/export`, {}));
  add("预览升级包", async () => { const value = await api(`/api/tickets/${selected.id}/escalation`); $("detail").textContent = JSON.stringify(value, null, 2); });
  selected.effects.filter((effect) => effect.status === "pending").forEach((effect) => {
    add(`批准 ${effect.path}`, () => act(`/api/tickets/${selected.id}/effects/${effect.id}/approval`, { approved: true }), "warn");
    add(`拒绝 ${effect.path}`, () => act(`/api/tickets/${selected.id}/effects/${effect.id}/approval`, { approved: false }));
  });
  selected.effects.filter((effect) => effect.status === "applied").forEach((effect) => add(`回滚 ${effect.path}`, () => act(`/api/tickets/${selected.id}/effects/${effect.id}/rollback`, {}), "warn"));
}

async function act(path, body) { try { notify(JSON.stringify(await api(path, { method: "POST", body: JSON.stringify(body) }))); await refresh(); } catch (error) { notify(error.message); } }
async function refresh() { state = await api("/api/state"); render(); }

$("project-form").onsubmit = async (event) => { event.preventDefault(); try { await api("/api/projects/select", { method: "POST", body: JSON.stringify({ path: $("project").value }) }); await refresh(); } catch (error) { notify(error.message); } };
$("ticket-form").onsubmit = async (event) => { event.preventDefault(); try { await api("/api/tickets", { method: "POST", body: JSON.stringify({ request: $("request").value }) }); $("request").value = ""; await refresh(); } catch (error) { notify(error.message); } };
api("/health").then((value) => $("health").textContent = value.model.status === "ready" ? "模型就绪" : "Agent 就绪 · 模型未发布").catch(() => $("health").textContent = "服务异常");
refresh(); setInterval(refresh, 3000);
