const canvas = document.getElementById("graph-canvas");
const ctx = canvas.getContext("2d");

const view = { x: 0, y: 0, scale: 1 };
const state = { nodes: [], edges: [], selected: null, dragging: false, last: null, truncated: false };

function sizeCanvas() {
  const ratio = window.devicePixelRatio || 1;
  const width = canvas.clientWidth || 900;
  const height = canvas.clientHeight || 560;
  canvas.width = width * ratio;
  canvas.height = height * ratio;
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
}

function toScreen(point) {
  return {
    x: (point.x - view.x) * view.scale + canvas.width / (2 * (window.devicePixelRatio || 1)),
    y: (point.y - view.y) * view.scale + canvas.height / (2 * (window.devicePixelRatio || 1)),
  };
}

function layout(nodes, centerId) {
  const cx = 0;
  const cy = 0;
  const radius = 180;
  return nodes.map((node, index) => {
    if (node.id === centerId) {
      return { ...node, x: cx, y: cy };
    }
    const angle = (index / Math.max(1, nodes.length - 1)) * Math.PI * 2;
    return { ...node, x: cx + Math.cos(angle) * radius, y: cy + Math.sin(angle) * radius };
  });
}

function draw() {
  sizeCanvas();
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  const positions = new Map(state.nodes.map((node) => [node.id, node]));

  ctx.strokeStyle = "#2c3240";
  ctx.fillStyle = "#8b93a5";
  ctx.lineWidth = 1;
  for (const edge of state.edges) {
    const from = positions.get(edge.from);
    const to = positions.get(edge.to);
    if (!from || !to) continue;
    const a = toScreen(from);
    const b = toScreen(to);
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
    ctx.fillText(edge.kind, (a.x + b.x) / 2, (a.y + b.y) / 2 - 2);
  }

  for (const node of state.nodes) {
    const point = toScreen(node);
    const isSelected = state.selected && node.id === state.selected.id;
    ctx.beginPath();
    ctx.arc(point.x, point.y, isSelected ? 8 : 6, 0, Math.PI * 2);
    ctx.fillStyle = node.is_test ? "#8b93a5" : isSelected ? "#6ea8fe" : "#4ec9a0";
    ctx.fill();
    if (view.scale >= 0.7) {
      ctx.fillStyle = "#d8dde7";
      ctx.fillText(node.name, point.x + 10, point.y + 4);
    }
  }
}

function detail(node) {
  const box = document.getElementById("graph-detail");
  if (!node) {
    box.innerHTML = "";
    return;
  }
  box.innerHTML =
    "<h3>" + node.name + "</h3>" +
    "<p class='muted'>" + node.qualified + "</p>" +
    "<pre>" + (node.signature || "") + "</pre>" +
    "<p class='muted'>" + node.file + ":" + node.start_line + "-" + node.end_line + "</p>" +
    "<p class='muted'>chamadores " + node.callers + " · chamados " + node.callees + "</p>";
}

function applyNeighborhood(payload) {
  state.truncated = payload.truncated;
  state.edges = payload.edges;
  state.nodes = layout(payload.nodes, payload.center.id);
  state.selected = state.nodes.find((node) => node.id === payload.center.id) || null;
  detail(state.selected);
  draw();
}

async function neighborhood(symbol) {
  const response = await fetch("/api/graph/neighborhood?symbol=" + encodeURIComponent(symbol) + "&depth=1&limit=120");
  if (!response.ok) {
    document.getElementById("graph-summary").textContent = "símbolo não encontrado: " + symbol;
    return;
  }
  applyNeighborhood(await response.json());
}

async function search(query) {
  const payload = await (await fetch("/api/graph/search?q=" + encodeURIComponent(query) + "&limit=40")).json();
  const list = document.getElementById("graph-results");
  list.innerHTML = payload.results
    .map((item) => '<li data-symbol="' + item.name + '">' + item.name + ' <span class="muted">' + item.file + ":" + item.start_line + "</span></li>")
    .join("") || '<li class="muted">nada encontrado</li>';
  list.querySelectorAll("li[data-symbol]").forEach((item) => {
    item.style.cursor = "pointer";
    item.addEventListener("click", () => neighborhood(item.dataset.symbol));
  });
}

async function trace(from, to) {
  const payload = await (await fetch("/api/graph/trace?from=" + encodeURIComponent(from) + "&to=" + encodeURIComponent(to))).json();
  const summary = document.getElementById("graph-summary");
  if (!payload.found) {
    summary.textContent = "nenhuma rota entre " + from + " e " + to;
    return;
  }
  summary.textContent = "rota: " + payload.path.join(" → ");
  state.edges = payload.edges;
  state.nodes = layout(payload.nodes, payload.nodes[0] ? payload.nodes[0].id : null);
  state.selected = state.nodes[0] || null;
  detail(state.selected);
  draw();
}

function pick(event) {
  const rect = canvas.getBoundingClientRect();
  const point = { x: event.clientX - rect.left, y: event.clientY - rect.top };
  let best = null;
  let bestDistance = 14;
  for (const node of state.nodes) {
    const screen = toScreen(node);
    const distance = Math.hypot(screen.x - point.x, screen.y - point.y);
    if (distance < bestDistance) {
      best = node;
      bestDistance = distance;
    }
  }
  return best;
}

canvas.addEventListener("click", (event) => {
  const node = pick(event);
  if (node) {
    state.selected = node;
    detail(node);
    draw();
  }
});

canvas.addEventListener("dblclick", (event) => {
  const node = pick(event);
  if (node) neighborhood(node.name);
});

canvas.addEventListener("mousedown", (event) => {
  state.dragging = true;
  state.last = { x: event.clientX, y: event.clientY };
});

window.addEventListener("mouseup", () => {
  state.dragging = false;
});

canvas.addEventListener("mousemove", (event) => {
  if (!state.dragging || !state.last) return;
  const dx = (event.clientX - state.last.x) / view.scale;
  const dy = (event.clientY - state.last.y) / view.scale;
  view.x -= dx;
  view.y -= dy;
  state.last = { x: event.clientX, y: event.clientY };
  draw();
});

canvas.addEventListener("wheel", (event) => {
  event.preventDefault();
  view.scale = Math.min(3, Math.max(0.3, view.scale * (event.deltaY < 0 ? 1.1 : 0.9)));
  draw();
});

export function initGraph() {
  sizeCanvas();
  draw();
  document.getElementById("graph-search-run").addEventListener("click", () => {
    search(document.getElementById("graph-search").value.trim());
  });
  document.getElementById("trace-run").addEventListener("click", () => {
    trace(
      document.getElementById("trace-from").value.trim(),
      document.getElementById("trace-to").value.trim(),
    );
  });
}

export async function refreshGraphSummary() {
  const summary = await (await fetch("/api/graph/summary")).json();
  document.getElementById("graph-summary").textContent =
    summary.node_count + " nós · " + summary.edge_count + " arestas · " + summary.file_count + " arquivos";
}

export function refreshHint() {
  refreshGraphSummary();
}
