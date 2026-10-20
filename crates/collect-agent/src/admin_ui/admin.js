// IP 명단 관리 창(에이전트 트레이). 요청은 window.ipc.postMessage({id, req})로 보내고 답은 mm.reply(id, 답)으로 온다.
// 참여자 이름은 다른 PC에서 들어온 값이라 화면에는 textContent로만 넣는다(innerHTML을 쓰지 않는다).
(() => {
  let seq = 0;
  const waiting = new Map();
  let view = null;
  let busy = false;

  const $ = (id) => document.getElementById(id);

  function send(message) {
    return new Promise((resolve) => {
      const id = ++seq;
      waiting.set(id, resolve);
      window.ipc.postMessage(JSON.stringify({ id, ...message }));
    });
  }

  /** 요청을 보내고 답의 상태로 다시 그린다. 실패하면 까닭을 알린다. */
  async function run(message) {
    busy = true;
    try {
      const res = await send(message);
      if (res.view) render(res.view);
      if (!res.ok) notify(res.message || "하지 못했습니다", true);
      else if (res.notice) notify(res.notice, false);
      return res.ok;
    } finally {
      busy = false;
    }
  }

  const call = (req) => run({ req });

  function notify(text, error) {
    const el = $("notice");
    el.textContent = text;
    el.className = error ? "error" : "";
    el.hidden = false;
    clearTimeout(notify.timer);
    notify.timer = setTimeout(() => (el.hidden = true), error ? 8000 : 4000);
  }

  function el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") node.className = v;
      else if (k === "text") node.textContent = v;
      else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v);
    }
    for (const c of children) node.append(c);
    return node;
  }

  const button = (text, onclick, cls = "") => el("button", { type: "button", class: cls, text, onclick });

  function render(next) {
    view = next;
    $("path").textContent = `명단 파일: ${view.path}`;
    $("agent-ips").textContent = view.agentIps.length ? `이 PC의 IP: ${view.agentIps.join(", ")}` : "";
    renderEntries();
    renderPending();
    renderOutsiders();
    renderReceived();
  }

  function renderEntries() {
    const q = $("search").value.trim().toLowerCase();
    const rows = view.entries.filter((e) => !q || e.ip.includes(q) || e.name.toLowerCase().includes(q));
    $("count").textContent = `${view.entries.length}명`;
    $("empty").hidden = view.entries.length > 0;
    const body = $("entries");
    body.replaceChildren(
      ...rows.map((e) => {
        const input = el("input", { value: e.name, maxlength: "20", "aria-label": `${e.ip} 이름` });
        const save = () => {
          const name = input.value.trim();
          if (name && name !== e.name) void call({ op: "put", ip: e.ip, name });
          else input.value = e.name;
        };
        input.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") input.blur();
          if (ev.key === "Escape") {
            input.value = e.name;
            input.blur();
          }
        });
        input.addEventListener("blur", save);
        const name = el("td", { class: "name" }, input);
        if (e.duplicate) name.append(el("span", { class: "dup", text: "같은 이름이 다른 IP에도 있어요" }));
        const online = e.online.length
          ? el("span", { class: "online", text: "● 접속 중", title: e.online.join(", ") })
          : el("span", { class: "muted", text: "–" });
        const remove = button("빼기", () => void call({ op: "remove", ip: e.ip }), "danger");
        return el(
          "tr",
          {},
          el("td", { class: "ip", text: e.ip }),
          name,
          el("td", {}, online),
          el("td", { class: "buttons" }, remove),
        );
      }),
    );
  }

  function reasonText(r) {
    if (r === "online") return "같은 이름이 다른 기기에서 접속 중";
    if (r === "approval") return "호스트가 끊었거나 거절한 이름";
    return "승인 필요";
  }

  function renderPending() {
    $("pending-section").hidden = view.pending.length === 0;
    $("pending").replaceChildren(
      ...view.pending.map((p) => {
        const approve = button(
          "승인",
          () => {
            if (
              p.reason === "online" &&
              !confirm(`‘${p.name}’(으)로 접속 중인 다른 기기의 연결이 끊깁니다. 승인할까요?`)
            )
              return;
            void call({ op: "approve", service: p.service, name: p.name });
          },
          "primary",
        );
        const reject = button("거부", () => void call({ op: "reject", service: p.service, name: p.name }));
        return el(
          "tr",
          {},
          el("td", { text: p.title || "새 취합" }),
          el("td", { text: p.name }),
          el("td", { class: "ip", text: p.ip }),
          el("td", { class: "muted", text: reasonText(p.reason) }),
          el("td", { class: "buttons" }, approve, reject),
        );
      }),
    );
  }

  function renderOutsiders() {
    $("outsiders-section").hidden = view.outsiders.length === 0;
    $("outsiders").replaceChildren(
      ...view.outsiders.map((o) => {
        const add = button("명단에 넣기", () => void call({ op: "put", ip: o.ip, name: o.name }));
        return el(
          "tr",
          {},
          el("td", { text: o.title || "새 취합" }),
          el("td", { text: o.name }),
          el("td", { class: "ip", text: o.ip }),
          el("td", { class: o.online ? "online" : "muted", text: o.online ? "● 접속 중" : "나감" }),
          el("td", { class: "buttons" }, add),
        );
      }),
    );
  }

  function renderReceived() {
    $("received-section").hidden = view.received.length === 0;
    const groups = new Map();
    for (const r of view.received) {
      if (!groups.has(r.service)) groups.set(r.service, []);
      groups.get(r.service).push(r);
    }
    $("received").replaceChildren(
      ...[...groups.entries()].map(([service, rows]) => {
        const all = button("모두 이 PC 명단에 넣기", () => void call({ op: "adoptReceived", service }));
        const head = el("div", { class: "group-head" }, el("b", { text: rows[0].title || "새 취합" }), all);
        const body = el(
          "tbody",
          {},
          ...rows.map((r) => {
            const adopt = button("이 PC 명단에 넣기", () => void call({ op: "adoptReceived", service, ip: r.ip }));
            adopt.disabled = r.shadowed;
            const remove = button(
              "이 업무에서 빼기",
              () => void call({ op: "removeReceived", service, ip: r.ip }),
              "danger",
            );
            return el(
              "tr",
              {},
              el("td", { class: `ip${r.shadowed ? " shadowed" : ""}`, text: r.ip }),
              el("td", { class: r.shadowed ? "shadowed" : "", text: r.name }),
              el("td", { class: "muted small", text: r.shadowed ? "이 PC 명단이 먼저" : "" }),
              el("td", { class: "buttons" }, adopt, remove),
            );
          }),
        );
        return el("div", { class: "group" }, head, el("table", {}, body));
      }),
    );
  }

  window.mm = {
    reply(id, res) {
      const resolve = waiting.get(id);
      waiting.delete(id);
      if (resolve) resolve(res);
    },
    /** 서비스·명단이 바뀌었다(트레이가 알린다). 고치는 중이 아니면 다시 읽는다. */
    changed() {
      if (!busy && !document.activeElement?.closest("td.name")) void call({ op: "state" });
    },
  };

  $("add").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const ip = $("add-ip").value.trim();
    const name = $("add-name").value.trim();
    if (!ip || !name) return notify("IP와 이름을 모두 적어 주세요", true);
    if (await call({ op: "put", ip, name })) {
      $("add-ip").value = "";
      $("add-name").value = "";
      $("add-ip").focus();
    }
  });
  $("search").addEventListener("input", () => view && renderEntries());
  $("import").addEventListener("click", () => void run({ pick: "import" }));
  $("export").addEventListener("click", () => void run({ pick: "export" }));

  // 보이는 동안 3초마다 접속 상태를 새로 읽는다.
  setInterval(() => {
    if (document.visibilityState === "visible") window.mm.changed();
  }, 3000);
  void call({ op: "state" });
})();
