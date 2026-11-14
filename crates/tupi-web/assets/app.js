import { initGraph, refreshGraphSummary, refreshHint } from "./graph.js";

const state = { context: { tokens: 0, budget: 0 }, timeline: [] };

const el = (id) => document.getElementById(id);

function text(value) {
  return value === undefined || value === null ? "—" : String(value);
}

function badge(status, label) {
  const node = el("stream-status");
  node.className = "badge " + status;
  node.textContent = label;
}

document.querySelectorAll("#tabs button").forEach((button) => {
  button.addEventListener("click", () => {
    document.querySelectorAll("#tabs button").forEach((item) => item.classList.remove("active"));
    document.querySelectorAll("main .panel").forEach((item) => item.classList.remove("active"));
    button.classList.add("active");
    el("panel-" + button.dataset.tab).classList.add("active");
  });
});

async function getJson(path) {
  const response = await fetch(path);
  if (!response.ok) {
    throw new Error(path + " → " + response.status);
  }
  return response.json();
}

async function loadOverview() {
  const overview = await getJson("/api/overview");
  el("session-model").textContent = text(overview.model);
  el("session-provider").textContent = text(overview.provider);
  renderCards(overview.stats);
  renderContext();
}

function renderCards(stats) {
  const cards = [
    ["turnos", stats.turns],
    ["requisições", stats.provider_requests],
    ["tool calls", stats.tool_calls],
    ["falhas de tool", stats.tool_failures],
    ["prompt tokens", stats.prompt_tokens],
    ["completion tokens", stats.completion_tokens],
    ["comandos", stats.commands],
    ["consultas", stats.queries],
  ];
  el("session-cards").innerHTML = cards
    .map(([label, value]) => '<div class="card"><div class="value">' + text(value) + '</div><div class="label">' + label + "</div></div>")
    .join("");
}

function renderContext() {
  const { tokens, budget } = state.context;
  el("context-usage").textContent = budget
    ? "contexto " + tokens + " / " + budget
    : "contexto " + tokens;
}

async function loadMemory() {
  const memory = await getJson("/api/memory");
  const fact = (item) =>
    '<li><span class="tag">' + (item.tags || []).join(",") + "</span>" + item.text + "</li>";
  const pending = (item) =>
    '<li><span class="tag">' + text(item.kind) + "</span>" + text(item.id) + " <span class=\"when\">" + text(item.status) + "</span></li>";
  el("facts-active").innerHTML = memory.active_facts.map(fact).join("") || '<li class="muted">nenhum fato</li>';
  el("facts-forgotten").innerHTML = memory.forgotten_facts.map(fact).join("") || '<li class="muted">nenhum</li>';
  el("pendings-open").innerHTML = memory.open_pendings.map(pending).join("") || '<li class="muted">nenhuma</li>';
  el("pendings-closed").innerHTML = memory.closed_pendings.map(pending).join("") || '<li class="muted">nenhuma</li>';
}

async function loadWorkspace() {
  const [entries, workflows, history] = await Promise.all([
    getJson("/api/state"),
    getJson("/api/workflows"),
    getJson("/api/history"),
  ]);
  el("state-entries").innerHTML = entries.entries
    .map((entry) => '<li><span class="tag">' + text(entry.kind) + "</span>" + text(entry.text) + "</li>")
    .join("") || '<li class="muted">nada registrado</li>';

  el("workflow-list").innerHTML = workflows.workflows
    .map((workflow) => '<li><span class="tag">' + text(workflow.source) + "</span>" + text(workflow.name) + "</li>")
    .join("") || '<li class="muted">nenhum workflow</li>';

  el("history-list").innerHTML = history.messages
    .slice(-30)
    .map((message) => '<li><span class="tag">' + text(message.role) + "</span>" + text(message.content).slice(0, 160) + "</li>")
    .join("") || '<li class="muted">sem mensagens</li>';
}

function pushTimeline(label, detail) {
  const item = document.createElement("li");
  const when = new Date().toLocaleTimeString();
  item.innerHTML = '<span class="when">' + when + "</span> " + label + (detail ? " · " + detail : "");
  state.timeline.unshift(item);
  state.timeline = state.timeline.slice(0, 200);
  el("timeline").replaceChildren(...state.timeline);
}

function handleEvent(event) {
  const type = event.type;
  switch (type) {
    case "token":
      break;
    case "stream_finished": {
      const usage = [event.prompt_tokens, event.cached_tokens, event.completion_tokens];
      pushTimeline("resposta concluída", "prompt/cache/completion: " + usage.map(text).join(" / "));
      loadOverview();
      break;
    }
    case "stream_failed":
      pushTimeline("falha no stream", event.message);
      break;
    case "context_updated":
      state.context = { tokens: event.tokens, budget: event.budget };
      renderContext();
      break;
    case "context_budget":
      // Troca de modelo: o teto muda sem nova ocupação; preserva os tokens.
      state.context = { tokens: state.context.tokens, budget: event.budget };
      renderContext();
      break;
    case "context_trimmed":
      pushTimeline("contexto podado", "omitidos " + event.omitted_units);
      break;
    case "tool_started":
      pushTimeline("tool " + event.name, event.summary);
      break;
    case "tool_finished":
      pushTimeline((event.ok ? "ok " : "erro ") + event.name, event.summary);
      break;
    case "memory":
      pushTimeline("memória", event.text);
      if (!event.after) loadMemory();
      break;
    case "notice":
      pushTimeline("aviso", event.message);
      break;
    case "models_loaded":
      pushTimeline("modelos", event.models.length + " disponíveis");
      break;
    case "refresh_hint":
      if (event.scope === "memory") loadMemory();
      else if (event.scope === "graph") refreshHint();
      else loadWorkspace();
      break;
    case "lagged":
      pushTimeline("eventos perdidos", event.skipped + " eventos");
      break;
    default:
      break;
  }
}

function connectEvents() {
  const source = new EventSource("/api/events");
  source.addEventListener("open", () => badge("online", "ao vivo"));
  source.addEventListener("error", () => badge("waiting", "reconectando"));
  source.addEventListener("update", (message) => {
    try {
      handleEvent(JSON.parse(message.data));
    } catch (error) {
      console.error(error);
    }
  });
}

initGraph();
loadOverview();
loadMemory();
loadWorkspace();
refreshGraphSummary();
connectEvents();
setInterval(loadOverview, 5000);

