'use strict';

const TAURI = window.__TAURI__;
const invoke = TAURI.core.invoke;
const listen = TAURI.event.listen;
const appWindow = TAURI.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const isUrl = (s) => /^https?:\/\/\S+$/i.test((s || '').trim());
const today = () => new Date().toISOString().slice(0, 10);

/* ---------- settings ---------- */

const DEFAULTS = {
  outDir: '', nameTpl: 'title_id', langs: ['ko', 'en'], subFmt: 'srt', thumbFmt: 'jpg',
  cookies: '', channel: 'stable', autoUpdate: true, lastCheck: '',
  quality: 'best', withSubs: true, withThumb: true, withDesc: false,
};
function loadJSON(key, fallback) {
  try { const v = localStorage.getItem(key); return v ? JSON.parse(v) : fallback; } catch { return fallback; }
}
function saveJSON(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* storage unavailable */ }
}
const settings = Object.assign({}, DEFAULTS, loadJSON('grabber.settings', {}));
const saveSettings = () => saveJSON('grabber.settings', settings);

/* ---------- state ---------- */

const S = {
  view: 'main',
  url: '',
  probe: null,
  probing: false,
  probeError: '',
  probeSeq: 0,
  clipNote: false,
  lastClip: '',
  qId: null,
  group: null,
  subs: new Set(),
  showAllSubs: false,
  thumb: settings.withThumb,
  desc: settings.withDesc,
  jobs: [],
  recent: loadJSON('grabber.recent', []),
  showAllRecent: false,
  tools: [],
  toolsChecked: false,
  toolProgress: {},
  engineBusy: false,
  engineMsg: '',
  appUpdate: null,
  appUpdateBusy: false,
  version: '',
  showAllLangs: false,
};

/* ---------- formatting ---------- */

function fmtBytes(n) {
  if (!n && n !== 0) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0; let v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
}
function fmtTime(sec) {
  if (sec == null || !isFinite(sec)) return '';
  sec = Math.round(sec);
  const h = Math.floor(sec / 3600); const m = Math.floor((sec % 3600) / 60); const s = sec % 60;
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}` : `${m}:${String(s).padStart(2, '0')}`;
}
const langNames = (() => { try { return new Intl.DisplayNames(['ko'], { type: 'language' }); } catch { return null; } })();
function langName(code, fallback) {
  const base = code.replace(/-orig$/, '');
  try { const n = langNames && langNames.of(base); if (n && n !== base) return n; } catch { /* odd code */ }
  return fallback || code;
}
const thumbStyle = (url) => (url ? `style="background-image:url('${esc(url).replace(/'/g, '%27')}')"` : '');

/* ---------- views ---------- */

function show(view) {
  S.view = view;
  for (const v of ['setup', 'main', 'queue', 'settings']) $(`view-${v}`).classList.toggle('hidden', v !== view);
  const inSettings = view === 'settings';
  $('brand').classList.toggle('hidden', inSettings);
  $('backTitle').classList.toggle('hidden', !inSettings);
  $('titlebar').classList.toggle('back', inSettings);
  $('btnSettings').classList.toggle('hidden', inSettings || view === 'setup');
  $('tbSep').classList.toggle('hidden', inSettings || view === 'setup');
  $('statusbar').classList.toggle('hidden', view === 'setup' || inSettings);
  render();
}

function activeJobs() { return S.jobs.filter((j) => j.status === 'running' || j.status === 'queued'); }

function render() {
  if (S.view === 'main') renderMain();
  if (S.view === 'queue') renderQueue();
  if (S.view === 'settings') renderSettings();
  if (S.view === 'setup') renderSetup();
  renderStatus();
}

/* ---------- compose (main) ---------- */

function selectedQuality() {
  return S.probe ? S.probe.qualities.find((q) => q.id === S.qId) : null;
}

function pickDefaultQuality(p) {
  const want = settings.quality;
  const byRole = (r) => p.qualities.find((q) => q.role === r);
  const q = (want === 'best' && byRole('best'))
    || (want === 'edit' && (byRole('edit') || byRole('best')))
    || p.qualities.find((x) => x.id === want)
    || byRole('best') || p.qualities[0];
  return q ? q.id : null;
}

function subRows(p) {
  if (p.kind === 'playlist') {
    return {
      primary: settings.langs.map((l) => ({ code: l, name: langName(l), auto: false, playlist: true })),
      rest: [],
    };
  }
  const rows = p.subs.map((s) => ({ ...s, base: s.code.split('-')[0] }));
  const hasOrig = rows.some((r) => r.code.endsWith('-orig'));
  rows.forEach((r) => {
    r.kindText = !r.auto ? '업로더 자막' : (r.code.endsWith('-orig') || !hasOrig ? '자동 생성' : '자동 번역');
  });
  const primary = [];
  const add = (r) => { if (r && !primary.includes(r)) primary.push(r); };
  for (const base of settings.langs) {
    add(rows.find((r) => !r.auto && (r.code === base || r.base === base))
      || rows.find((r) => r.auto && r.code === `${base}-orig`)
      || rows.find((r) => r.auto && r.code === base));
  }
  rows.filter((r) => r.auto && r.code.endsWith('-orig')).forEach(add);
  rows.filter((r) => !r.auto).forEach(add);
  return { primary, rest: rows.filter((r) => !primary.includes(r)) };
}

function defaultSubs(p) {
  const set = new Set();
  if (!settings.withSubs) return set;
  const { primary } = subRows(p);
  for (const r of primary) {
    const base = r.code.split('-')[0];
    // machine-translated captions are often refused by YouTube (HTTP 429), so leave them unticked
    if (settings.langs.includes(base) && r.kindText !== '자동 번역') set.add(r.code);
  }
  return set;
}

function renderPreview() {
  const box = $('preview');
  if (S.probing) {
    box.innerHTML = `<div class="preview"><div class="thumb skeleton"></div>
      <div class="meta"><div class="skeleton" style="height:22px;width:70%"></div><div class="skeleton" style="height:14px;width:30%"></div>
      <span class="hint">영상 정보를 불러오는 중…</span></div></div>`;
    return;
  }
  if (S.probeError) {
    box.innerHTML = `<div class="error-text">${esc(S.probeError)}</div>
      <div style="display:flex;gap:10px;margin-top:12px;flex-wrap:wrap">
        <button class="btn sm" data-act="reprobe">다시 시도</button>
        ${/엔진/.test(S.probeError) ? '<button class="btn sm" data-act="engine-update">엔진 업데이트</button>' : ''}
      </div>`;
    return;
  }
  const p = S.probe;
  if (!p) {
    box.innerHTML = '<div class="empty">유튜브 링크를 붙여넣으면 영상 정보와 받을 수 있는 화질이 여기에 나타나요.<br>앱을 열 때 클립보드에 링크가 있으면 자동으로 가져와요.</div>';
    return;
  }
  const sub = p.kind === 'playlist' ? `재생목록 · ${p.count ?? '?'}개 영상` : esc(p.channel);
  box.innerHTML = `<div class="preview">
      <div class="thumb" ${thumbStyle(p.thumbnail)}>${p.duration ? `<span class="dur">${fmtTime(p.duration)}</span>` : ''}</div>
      <div class="meta"><h2 class="selectable">${esc(p.title)}</h2><div class="muted" style="font-size:13px">${sub}</div></div>
    </div>`;
}

function renderOptions() {
  const p = S.probe;
  $('options').classList.toggle('hidden', !p || S.probing);
  if (!p) return;

  const groups = [];
  for (const q of p.qualities) if (!groups.includes(q.group)) groups.push(q.group);
  if (!groups.includes(S.group)) S.group = groups[0];
  $('resList').innerHTML = groups.map((g) => {
    const sel = g === S.group;
    const codecs = [...new Set(p.qualities.filter((q) => q.group === g).map((q) => q.codec))].join(' · ');
    return `<button class="row${sel ? ' sel' : ''}" aria-pressed="${sel}" data-act="res" data-id="${esc(g)}" style="min-height:56px">
      <span class="radio" aria-hidden="true"></span>
      <span class="main"><span class="mono" style="font-weight:500">${esc(g)}</span><span class="sub ellipsis">${esc(codecs)}</span></span>
    </button>`;
  }).join('');

  $('qualityList').innerHTML = p.qualities.filter((q) => q.group === S.group).map((q) => {
    const sel = q.id === S.qId;
    const tag = q.role === 'best' ? '<span class="tag best">최고 화질</span>' : q.role === 'edit' ? '<span class="tag edit">편집 호환</span>' : '';
    return `<button class="row${sel ? ' sel' : ''}" aria-pressed="${sel}" data-act="quality" data-id="${esc(q.id)}" style="min-height:56px">
      <span class="radio" aria-hidden="true"></span>
      <span class="main">
        <span style="display:flex;align-items:center;gap:8px;min-width:0"><span class="codecname">${esc(q.codec)}</span>${tag}</span>
        ${q.detail ? `<span class="sub ellipsis">${esc(q.detail)}</span>` : ''}
      </span>
      <span class="ext">${esc(q.ext || '')}</span>
      <span class="size">${q.size ? '약 ' + fmtBytes(q.size) : ''}</span>
    </button>`;
  }).join('');

  const { primary, rest } = subRows(p);
  const subRow = (r) => {
    const on = S.subs.has(r.code);
    return `<button class="row${on ? ' on' : ''}" aria-pressed="${on}" data-act="sub" data-code="${esc(r.code)}">
      <span class="check" aria-hidden="true"></span>
      <span class="main">${esc(langName(r.code, r.name))}</span>
      <span class="side">${r.playlist ? '있으면 받기' : esc(r.kindText)}</span></button>`;
  };
  let subsHtml = primary.length ? primary.map(subRow).join('') : '<div class="row"><span class="muted" style="font-size:13px">이 영상에는 자막이 없어요</span></div>';
  if (rest.length) {
    subsHtml += S.showAllSubs
      ? `<div class="list scrolly" style="border:0;border-radius:0;border-top:1px solid var(--line)">${rest.map(subRow).join('')}</div>
         <a class="row link" data-act="subs-less">접기</a>`
      : `<a class="row link" data-act="subs-more">다른 언어 보기 · ${rest.length}개</a>`;
  }
  $('subList').innerHTML = subsHtml;
  $('subFmtHint').textContent = settings.subFmt === 'embed' ? '영상에 넣기' : `${settings.subFmt.toUpperCase()}로 저장`;

  const extra = (id, on, label, side) => `<button class="row${on ? ' on' : ''}" aria-pressed="${on}" data-act="${id}">
      <span class="check" aria-hidden="true"></span><span class="main">${label}</span><span class="side">${side}</span></button>`;
  const tf = { jpg: '최고 해상도 JPG', png: '최고 해상도 PNG', orig: '원본 형식' }[settings.thumbFmt];
  $('extraList').innerHTML = extra('thumb', S.thumb, '썸네일', tf) + extra('desc', S.desc, '영상 설명', '텍스트');
}

function renderMain() {
  $('clipNote').classList.toggle('hidden', !S.clipNote);
  renderPreview();
  renderOptions();
  const q = selectedQuality();
  $('btnDownload').disabled = !S.probe || !q || S.probing;
  const active = activeJobs().length;
  $('btnDownloadText').textContent = active ? '대기열에 추가' : '다운로드';
  $('outDirText').textContent = settings.outDir;
  const lq = $('lnkQueue');
  lq.classList.toggle('hidden', !S.jobs.length);
  lq.textContent = active ? `다운로드 중 ${active}개 보기 →` : '이번에 받은 목록 보기 →';
  renderRecent();
}

function renderRecent() {
  const list = S.showAllRecent ? S.recent : S.recent.slice(0, 3);
  $('recentBox').classList.toggle('hidden', !S.recent.length);
  $('lnkRecentAll').textContent = S.showAllRecent ? '접기' : '모두 보기';
  $('lnkRecentAll').classList.toggle('hidden', S.recent.length <= 3);
  $('recentList').innerHTML = list.map((r, i) => `<div class="file">
      <span class="thumb sm" ${thumbStyle(r.thumbnail)}></span>
      <div class="main"><span class="ellipsis">${esc(r.title)}</span><span class="sub ellipsis" style="font-size:12px;color:var(--muted)">${esc(r.summary)}</span></div>
      <button class="btn" data-act="reveal-recent" data-i="${i}">폴더 열기</button></div>`).join('');
}

/* ---------- probing ---------- */

let probeTimer = null;
function scheduleProbe(delay = 350) {
  clearTimeout(probeTimer);
  probeTimer = setTimeout(() => runProbe(), delay);
}

async function runProbe(force = false) {
  const url = $('url').value.trim();
  if (!isUrl(url)) {
    if (!url) { S.probe = null; S.probeError = ''; S.probing = false; S.url = ''; render(); }
    return;
  }
  if (!force && url === S.url && (S.probe || S.probing)) return;
  S.url = url;
  const seq = ++S.probeSeq;
  S.probing = true; S.probeError = ''; S.probe = null; S.showAllSubs = false;
  render();
  try {
    const p = await invoke('probe', { url, cookies: settings.cookies || null });
    if (seq !== S.probeSeq) return;
    S.probe = p;
    S.qId = pickDefaultQuality(p);
    S.group = (p.qualities.find((q) => q.id === S.qId) || p.qualities[0] || {}).group;
    S.subs = defaultSubs(p);
    S.thumb = settings.withThumb;
    S.desc = settings.withDesc;
  } catch (e) {
    if (seq !== S.probeSeq) return;
    S.probeError = String(e);
  } finally {
    if (seq === S.probeSeq) { S.probing = false; render(); }
  }
}

/* ---------- jobs ---------- */

function summaryFor(q, subsCount, thumb) {
  const parts = [q.kind === 'video' ? `${q.group} ${q.codec} ${q.ext || ''}`.trim() : (q.ext || q.kind.toUpperCase())];
  if (subsCount) parts.push(`자막 ${subsCount}개`);
  if (thumb) parts.push('썸네일');
  return parts.join(' · ');
}

function enqueue() {
  const p = S.probe; const q = selectedQuality();
  if (!p || !q) return;
  const subCodes = [...S.subs];
  const subObjs = p.kind === 'playlist'
    ? subCodes.map((c) => ({ code: c, auto: false, playlist: true }))
    : p.subs.filter((s) => S.subs.has(s.code));
  const id = (crypto.randomUUID && crypto.randomUUID()) || String(Date.now() + Math.random());
  const payload = {
    id,
    url: p.url,
    videoId: p.kind === 'video' ? p.id : null,
    format: q.format,
    kind: q.kind,
    container: q.container || 'auto',
    subLangs: subCodes,
    writeSubs: subObjs.some((s) => !s.auto),
    writeAutoSubs: subObjs.some((s) => s.auto || s.playlist),
    subFormat: settings.subFmt,
    thumbnail: S.thumb,
    thumbFormat: settings.thumbFmt,
    description: S.desc,
    outDir: settings.outDir,
    nameTpl: settings.nameTpl,
    cookies: settings.cookies || null,
    playlist: p.kind === 'playlist',
  };
  S.jobs.push({
    id, payload, status: 'queued', stage: 'info',
    title: p.title, channel: p.kind === 'playlist' ? `재생목록 · ${p.count ?? '?'}개` : p.channel,
    thumbnail: p.thumbnail, kind: q.kind,
    qLabel: [q.group === '음원만' ? '' : q.group, q.codec, q.detail, q.ext].filter(Boolean).join(' · '),
    summary: summaryFor(q, subCodes.length, S.thumb), size: q.size,
  });
  // remember choices for next time
  settings.quality = q.role || q.id;
  settings.withThumb = S.thumb;
  settings.withDesc = S.desc;
  if (p.kind === 'video' && subRows(p).primary.length) settings.withSubs = S.subs.size > 0;
  saveSettings();

  $('url').value = ''; S.url = ''; S.probe = null; S.clipNote = false;
  pump();
  show('queue');
}

async function pump() {
  if (S.jobs.some((j) => j.status === 'running')) return;
  const next = S.jobs.find((j) => j.status === 'queued');
  if (!next) { render(); return; }
  next.status = 'running'; next.stage = 'info';
  render();
  try {
    await invoke('start_download', { job: next.payload });
  } catch (e) {
    next.status = 'error'; next.error = String(e);
    pump();
  }
}

function onProgress(ev) {
  const j = S.jobs.find((x) => x.id === ev.id);
  if (!j || j.status !== 'running') return;
  Object.assign(j, {
    stage: ev.stage || j.stage,
    percent: ev.percent ?? j.percent,
    downloaded: ev.downloaded ?? j.downloaded,
    total: ev.total ?? j.total,
    speed: ev.speed ?? j.speed,
    eta: ev.eta ?? j.eta,
    item: ev.item ?? j.item,
    items: ev.items ?? j.items,
  });
  if (ev.stage === 'finish') j.percent = 100;
  if (S.view === 'queue') renderActiveOnly(j);
}

function onDone(ev) {
  const j = S.jobs.find((x) => x.id === ev.id);
  if (!j) return;
  j.file = ev.file || null;
  j.warning = ev.warning || '';
  if (ev.ok) {
    j.status = 'done';
    S.recent.unshift({
      title: j.title, thumbnail: j.thumbnail, summary: j.summary + (j.size ? ` · ${fmtBytes(j.size)}` : ''),
      file: j.file, dir: j.payload.outDir, at: Date.now(),
    });
    S.recent = S.recent.slice(0, 30);
    saveJSON('grabber.recent', S.recent);
  } else if (ev.cancelled) {
    j.status = 'cancelled';
  } else {
    j.status = 'error'; j.error = ev.error || '다운로드에 실패했어요.';
  }
  pump();
  render();
}

/* ---------- queue view ---------- */

function steps(j) {
  const list = j.kind === 'video' ? ['정보 확인', '영상', '음성', '마무리'] : ['정보 확인', '음원', '마무리'];
  const idx = j.stage === 'finish' ? list.length - 1
    : j.stage === 'audio' ? (j.kind === 'video' ? 2 : 1)
      : j.stage === 'video' ? 1 : 0;
  return list.map((s, i) => `<li class="${i < idx ? 'done' : i === idx ? 'now' : ''}">${s}</li>`).join('');
}

function activeCard(j) {
  const pct = j.percent != null ? Math.max(0, Math.min(100, j.percent)) : null;
  const indet = pct == null || j.stage === 'info' || j.stage === 'finish';
  const stageText = { info: '영상 정보 확인 중', video: '영상 받는 중', audio: j.kind === 'video' ? '음성 받는 중' : '음원 받는 중', finish: '파일 정리 중' }[j.stage] || '';
  const item = j.items ? ` · 항목 ${j.item}/${j.items}` : '';
  const left = indet ? `<b>${stageText}</b>${item}`
    : `<b>${pct.toFixed(0)}%</b> · ${fmtBytes(j.downloaded)}${j.total ? ' / ' + fmtBytes(j.total) : ''}${item}`;
  const right = indet ? '' : `${j.speed ? fmtBytes(j.speed) + '/s' : ''}${j.eta != null ? ' · 남은 시간 ' + fmtTime(j.eta) : ''}`;
  return `<section class="card active" id="active-${esc(j.id)}" aria-label="진행 중">
    <div class="active-top">
      <div class="thumb md" ${thumbStyle(j.thumbnail)}></div>
      <div class="meta">
        <div style="display:flex;align-items:flex-start;justify-content:space-between;gap:12px">
          <h2 class="selectable">${esc(j.title)}</h2>
          <button class="btn sm outline" data-act="cancel" data-id="${esc(j.id)}">취소</button>
        </div>
        <div class="muted" style="font-size:13px">${esc(j.channel)} · ${esc(j.qLabel)}</div>
        <ol class="steps">${steps(j)}</ol>
      </div>
    </div>
    <div style="display:flex;flex-direction:column;gap:8px">
      <div class="bar${indet ? ' indet' : ''}" role="progressbar" aria-label="다운로드 진행률" aria-valuemin="0" aria-valuemax="100" ${pct != null ? `aria-valuenow="${pct.toFixed(0)}"` : ''}>
        <div style="width:${pct ?? 0}%"></div>
      </div>
      <div class="stats"><span>${left}</span><span>${right}</span></div>
    </div>
  </section>`;
}

function renderActiveOnly(j) {
  const el = document.getElementById(`active-${j.id}`);
  if (!el) { renderQueue(); return; }
  const tmp = document.createElement('div');
  tmp.innerHTML = activeCard(j);
  el.replaceWith(tmp.firstElementChild);
}

function renderQueue() {
  const running = S.jobs.find((j) => j.status === 'running');
  const queued = S.jobs.filter((j) => j.status === 'queued');
  const failed = S.jobs.filter((j) => j.status === 'error');
  const done = S.jobs.filter((j) => j.status === 'done');
  let html = '';
  if (running) html += activeCard(running);
  if (!running && !queued.length) {
    html += `<div class="card" style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap">
      <span>${done.length ? '모든 다운로드가 끝났어요.' : '진행 중인 다운로드가 없어요.'}</span>
      <button class="btn-primary sm" data-act="new">새 다운로드</button></div>`;
  }
  if (queued.length) {
    html += `<section class="section"><div class="label-row"><h3 class="label">대기 중 ${queued.length}개</h3><a data-act="cancel-all">모두 취소</a></div>
      ${queued.map((j) => `<div class="qitem">
        <span class="thumb sm" ${thumbStyle(j.thumbnail)}></span>
        <div class="main" style="flex:1 1 auto;min-width:0;display:flex;flex-direction:column;gap:2px">
          <span class="ellipsis">${esc(j.title)}</span><span class="ellipsis" style="font-size:12px;color:var(--muted)">${esc(j.summary)}</span></div>
        <span class="pill">대기</span>
        <button class="icon-btn" aria-label="대기열에서 빼기" data-act="remove" data-id="${esc(j.id)}"><svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg></button>
      </div>`).join('')}</section>`;
  }
  if (failed.length) {
    html += `<section class="section"><h3 class="label">실패 ${failed.length}개</h3>
      ${failed.map((j) => `<div class="qitem" style="align-items:flex-start">
        <span class="thumb sm" ${thumbStyle(j.thumbnail)}></span>
        <div style="flex:1 1 auto;min-width:0;display:flex;flex-direction:column;gap:4px">
          <span class="ellipsis">${esc(j.title)}</span><span class="error-text" style="font-size:12px">${esc(j.error)}</span></div>
        <div style="display:flex;gap:8px;flex-wrap:wrap;justify-content:flex-end">
          ${/엔진/.test(j.error || '') ? '<button class="btn sm" data-act="engine-update">엔진 업데이트</button>' : ''}
          <button class="btn sm" data-act="retry" data-id="${esc(j.id)}">다시 시도</button>
          <button class="icon-btn" aria-label="목록에서 지우기" data-act="remove" data-id="${esc(j.id)}"><svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg></button>
        </div></div>`).join('')}</section>`;
  }
  if (done.length) {
    html += `<section class="files"><h3 class="label" style="margin-bottom:6px">완료</h3>
      ${done.map((j) => `<div class="file">
        <span class="thumb sm" ${thumbStyle(j.thumbnail)}></span>
        <div class="main"><span class="ellipsis">${esc(j.title)}</span><span class="ellipsis" style="font-size:12px;color:var(--muted)">${esc(j.summary)}</span>
          ${j.warning ? `<span style="font-size:12px;color:var(--warn);line-height:1.5">${esc(j.warning)}</span>` : ''}</div>
        <button class="btn" data-act="reveal-job" data-id="${esc(j.id)}">폴더 열기</button></div>`).join('')}</section>`;
  }
  $('queueBody').innerHTML = html;
}

/* ---------- engine / tools ---------- */

async function refreshTools(checkLatest) {
  try {
    S.tools = await invoke('tools_status', { channel: settings.channel, checkLatest });
    if (checkLatest) S.toolsChecked = true;
  } catch { /* keep old */ }
  render();
}

async function updateEngine({ silent = false } = {}) {
  if (S.engineBusy) return;
  if (activeJobs().length) {
    if (!silent) { S.engineMsg = '다운로드가 끝난 뒤 업데이트할 수 있어요.'; render(); }
    return;
  }
  S.engineBusy = true; S.engineMsg = ''; S.toolProgress = {};
  render();
  try {
    S.tools = await invoke('update_engine', { channel: settings.channel });
    S.toolsChecked = true;
    settings.lastCheck = today(); saveSettings();
    if (!silent) S.engineMsg = '업데이트를 마쳤어요.';
  } catch (e) {
    S.engineMsg = `업데이트 실패: ${e}`;
  } finally {
    S.engineBusy = false; S.toolProgress = {};
    render();
  }
}

/* ---------- settings view ---------- */

const LANGS = ['ko', 'en', 'ja', 'zh-Hans', 'zh-Hant', 'es', 'fr', 'de', 'vi', 'th', 'id', 'ru'];
const TOOL_DESC = { 'yt-dlp': '다운로드 엔진', ffmpeg: '영상·음성 합치기, 자막·썸네일 변환', deno: '유튜브 보안 처리용 JS 런타임' };
const TOOL_LABEL = { 'yt-dlp': 'yt-dlp', ffmpeg: 'ffmpeg', deno: 'Deno' };

function radioList(items, current, act) {
  return items.map((it) => {
    const sel = it.id === current;
    return `<button class="row${sel ? ' sel' : ''}" aria-pressed="${sel}" data-act="${act}" data-id="${it.id}">
      <span class="radio" aria-hidden="true"></span>
      <span class="main"><span>${it.label}</span>${it.desc ? `<span class="sub">${it.desc}</span>` : ''}</span>
      ${it.tag ? `<span class="tag best">${it.tag}</span>` : ''}</button>`;
  }).join('');
}

function renderSettings() {
  $('toolList').innerHTML = (S.tools.length ? S.tools : ['yt-dlp', 'ffmpeg', 'deno'].map((name) => ({ name }))).map((t) => {
    const prog = S.toolProgress[t.name];
    let badge;
    if (prog && prog.stage !== 'done') {
      badge = `<span class="badge muted">${prog.stage === 'download' ? `받는 중 ${Math.round(prog.percent)}%` : prog.stage === 'extract' ? '압축 푸는 중' : '확인 중'}</span>`;
    } else if (!t.installed) badge = '<span class="badge warn">설치 필요</span>';
    else if (t.outdated) badge = '<span class="badge warn">업데이트 있음</span>';
    else if (S.toolsChecked) badge = '<span class="badge ok">최신</span>';
    else badge = '<span class="badge muted">확인 중</span>';
    return `<div class="tool">
      <div style="flex:1 1 auto;min-width:0;display:flex;flex-direction:column;gap:2px">
        <span class="name">${TOOL_LABEL[t.name]}</span><span class="muted" style="font-size:12px">${TOOL_DESC[t.name]}</span></div>
      <span class="mono muted selectable" style="font-size:12px">${esc(t.version || '')}</span>${badge}</div>`;
  }).join('');
  $('btnEngineUpdate').disabled = S.engineBusy;
  $('btnEngineUpdateText').textContent = S.engineBusy ? '업데이트 중…' : '지금 업데이트';
  $('engineSub').textContent = S.engineMsg || '유튜브가 바뀌면 엔진 업데이트로 해결돼요';

  const on = settings.autoUpdate;
  $('autoList').innerHTML = `<button class="row${on ? ' on' : ''}" aria-pressed="${on}" data-act="auto-update" style="min-height:56px">
    <span class="check" aria-hidden="true"></span>
    <span class="main"><span>실행할 때 자동으로 확인</span><span class="sub">하루 한 번, 백그라운드에서</span></span></button>`;
  $('channelList').innerHTML = radioList([
    { id: 'stable', label: '안정판', desc: '검증된 정식 버전', tag: '권장' },
    { id: 'nightly', label: '나이틀리', desc: '매일 빌드 · 다운로드가 갑자기 안 될 때' },
  ], settings.channel, 'channel');

  $('setOutDir').textContent = settings.outDir;
  $('selName').value = settings.nameTpl;
  $('selCookie').value = settings.cookies;

  const visible = S.showAllLangs ? LANGS : LANGS.filter((l, i) => i < 3 || settings.langs.includes(l));
  $('langList').innerHTML = visible.map((l) => {
    const sel = settings.langs.includes(l);
    return `<button class="row${sel ? ' on' : ''}" aria-pressed="${sel}" data-act="lang" data-id="${l}" style="min-height:48px">
      <span class="check" aria-hidden="true"></span><span class="main">${esc(langName(l))}</span><span class="side mono">${l}</span></button>`;
  }).join('') + `<a class="row link" data-act="langs-toggle">${S.showAllLangs ? '접기' : '+ 언어 추가'}</a>`;

  $('subFmtList').innerHTML = radioList([
    { id: 'srt', label: 'SRT', desc: '편집 프로그램 호환' },
    { id: 'vtt', label: 'VTT', desc: '웹 플레이어용' },
    { id: 'embed', label: '영상에 넣기', desc: '파일 안에 자막 트랙으로' },
  ], settings.subFmt, 'sub-fmt');
  $('thumbFmtList').innerHTML = radioList([
    { id: 'jpg', label: 'JPG', desc: '어디서나 열림' },
    { id: 'png', label: 'PNG', desc: '무손실 · 용량 큼' },
    { id: 'orig', label: '원본', desc: '유튜브 원본 형식 그대로' },
  ], settings.thumbFmt, 'thumb-fmt');

  let info = `Grabber <span class="mono">${esc(S.version)}</span>`;
  if (S.appUpdateErr) info += ' · 업데이트 서버에 연결하지 못했어요';
  else if (S.appUpdate === false) info += ' · 최신 버전이에요';
  else if (S.appUpdate) info += ` · 새 버전 <span class="mono">${esc(S.appUpdate)}</span>이 있어요`;
  else info += ' · 새 버전이 나오면 알려드려요';
  $('appInfo').innerHTML = info;
  $('btnAppUpdate').textContent = S.appUpdateBusy ? '확인 중…' : S.appUpdate ? '설치하고 다시 시작' : '업데이트 확인';
  $('btnAppUpdate').disabled = S.appUpdateBusy;
}

/* ---------- setup ---------- */

function renderSetup() {
  $('setupSteps').innerHTML = ['yt-dlp', 'ffmpeg', 'deno'].map((name) => {
    const t = S.tools.find((x) => x.name === name);
    const p = S.toolProgress[name];
    const done = (p && p.stage === 'done') || (t && t.installed && !p);
    const now = p && p.stage !== 'done';
    const right = done ? '<span style="color:var(--ok);font-size:13px">완료</span>'
      : now ? `<span class="mono" style="font-size:13px">${p.stage === 'extract' ? '압축 푸는 중' : Math.round(p.percent) + '%'}</span>`
        : '<span class="muted" style="font-size:13px">대기</span>';
    const bar = now ? `<div class="bar thin${p.stage === 'extract' ? ' indet' : ''}" role="progressbar" aria-label="${TOOL_LABEL[name]} 받는 중" aria-valuenow="${Math.round(p.percent)}" aria-valuemin="0" aria-valuemax="100"><div style="width:${p.percent}%"></div></div>` : '';
    return `<div class="step-card${now ? ' now' : ''}"><div class="top"><span class="mono" style="font-weight:500">${TOOL_LABEL[name]}</span>${right}</div>${bar}</div>`;
  }).join('');
}

async function runSetup() {
  show('setup');
  $('setupError').classList.add('hidden');
  try {
    S.tools = await invoke('install_tools', { channel: settings.channel });
    settings.lastCheck = today(); saveSettings();
    S.toolProgress = {};
    return true;
  } catch (e) {
    $('setupErrorText').textContent = `도구를 받지 못했어요. 인터넷 연결을 확인하고 다시 시도해 주세요.\n${e}`;
    $('setupError').classList.remove('hidden');
    return false;
  }
}

/* ---------- status bar ---------- */

function renderStatus() {
  const left = $('statusLeft'); const right = $('statusRight');
  if (S.view === 'queue') {
    const c = (s) => S.jobs.filter((j) => j.status === s).length;
    left.textContent = `전체 ${S.jobs.length}개 · 진행 ${c('running')} · 대기 ${c('queued')} · 완료 ${c('done')}${c('error') ? ` · 실패 ${c('error')}` : ''}`;
  } else if (S.engineBusy) {
    left.innerHTML = '<span class="dot busy"></span>다운로드 엔진 업데이트 중…';
  } else if (S.tools.some((t) => t.outdated)) {
    left.innerHTML = '<span class="dot warn"></span>엔진 업데이트가 있어요 · <a data-act="engine-update">지금 업데이트</a>';
  } else {
    left.innerHTML = `<span class="dot"></span>다운로드 엔진 ${settings.lastCheck === today() ? '최신 상태 · 오늘 확인함' : '준비됨'}`;
  }
  right.innerHTML = S.appUpdate ? `<a data-act="app-update">새 버전 ${esc(S.appUpdate)} 설치</a>` : (S.version ? `v${esc(S.version)}` : '');
}

/* ---------- clipboard ---------- */

async function checkClipboard() {
  if (S.view !== 'main' && S.view !== 'queue') return;
  let text = '';
  try { text = (await invoke('read_clipboard')) || ''; } catch { return; }
  text = text.trim();
  if (!isUrl(text) || text === S.lastClip) return;
  S.lastClip = text;
  if (S.view === 'queue') {
    if (!$('addUrl').value) $('addUrl').value = text;
    return;
  }
  if (S.probing || (S.probe && $('url').value.trim() !== '')) return;
  $('url').value = text;
  S.clipNote = true;
  runProbe();
}

/* ---------- events ---------- */

document.addEventListener('click', async (e) => {
  const el = e.target.closest('[data-act]');
  if (!el) return;
  const act = el.dataset.act; const id = el.dataset.id;
  const job = id ? S.jobs.find((j) => j.id === id) : null;
  switch (act) {
    case 'quality': S.qId = id; renderMain(); break;
    case 'res': {
      // keep the same codec when it exists at the new resolution
      const cur = selectedQuality();
      const inGroup = S.probe.qualities.filter((q) => q.group === id);
      const same = cur && inGroup.find((q) => q.codec === cur.codec && q.id.endsWith('-hdr') === cur.id.endsWith('-hdr'));
      S.group = id;
      S.qId = (same || inGroup[0] || {}).id;
      renderMain(); break;
    }
    case 'sub': {
      const code = el.dataset.code;
      if (S.subs.has(code)) S.subs.delete(code); else S.subs.add(code);
      renderOptions(); break;
    }
    case 'subs-more': S.showAllSubs = true; renderOptions(); break;
    case 'subs-less': S.showAllSubs = false; renderOptions(); break;
    case 'thumb': S.thumb = !S.thumb; renderOptions(); break;
    case 'desc': S.desc = !S.desc; renderOptions(); break;
    case 'reprobe': runProbe(true); break;
    case 'engine-update': show('settings'); updateEngine(); break;
    case 'app-update': installAppUpdate(); break;
    case 'new': show('main'); $('url').focus(); break;
    case 'cancel': if (job) await invoke('cancel_download', { id: job.id }).catch(() => {}); break;
    case 'cancel-all':
      S.jobs = S.jobs.filter((j) => j.status !== 'queued');
      render(); break;
    case 'remove': S.jobs = S.jobs.filter((j) => j.id !== id); render(); break;
    case 'retry': if (job) { job.status = 'queued'; job.error = ''; job.percent = null; pump(); render(); } break;
    case 'reveal-job': if (job) invoke('reveal', { file: job.file, dir: job.payload.outDir }).catch(() => {}); break;
    case 'reveal-recent': {
      const r = (S.showAllRecent ? S.recent : S.recent.slice(0, 3))[Number(el.dataset.i)];
      if (r) invoke('reveal', { file: r.file, dir: r.dir || settings.outDir }).catch(() => {});
      break;
    }
    case 'auto-update': settings.autoUpdate = !settings.autoUpdate; saveSettings(); render(); break;
    case 'channel':
      if (settings.channel !== id) { settings.channel = id; saveSettings(); render(); updateEngine(); }
      break;
    case 'lang': {
      const i = settings.langs.indexOf(id);
      if (i >= 0) settings.langs.splice(i, 1); else settings.langs.push(id);
      saveSettings(); render(); break;
    }
    case 'langs-toggle': S.showAllLangs = !S.showAllLangs; render(); break;
    case 'sub-fmt': settings.subFmt = id; saveSettings(); render(); break;
    case 'thumb-fmt': settings.thumbFmt = id; saveSettings(); render(); break;
    default: break;
  }
});

$('url').addEventListener('input', () => { S.clipNote = false; $('clipNote').classList.add('hidden'); scheduleProbe(); });
$('url').addEventListener('keydown', (e) => {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  if (S.probe && !S.probing && $('url').value.trim() === S.url) enqueue();
  else runProbe(true);
});
$('btnClear').addEventListener('click', () => {
  $('url').value = ''; S.url = ''; S.probe = null; S.probeError = ''; S.clipNote = false; S.probeSeq++; S.probing = false;
  render(); $('url').focus();
});
$('btnDownload').addEventListener('click', enqueue);
$('lnkQueue').addEventListener('click', () => show('queue'));
$('lnkRecentAll').addEventListener('click', () => { S.showAllRecent = !S.showAllRecent; renderRecent(); });
$('lnkOpenDir').addEventListener('click', () => invoke('reveal', { file: null, dir: settings.outDir }).catch(() => {}));

async function changeDir() {
  const dir = await invoke('pick_folder').catch(() => null);
  if (dir) { settings.outDir = dir; saveSettings(); render(); }
}
$('lnkChangeDir').addEventListener('click', changeDir);
$('btnSetDir').addEventListener('click', changeDir);
$('selName').addEventListener('change', (e) => { settings.nameTpl = e.target.value; saveSettings(); });
$('selCookie').addEventListener('change', (e) => { settings.cookies = e.target.value; saveSettings(); });

function addFromQueue() {
  const url = $('addUrl').value.trim();
  if (!isUrl(url)) { $('addUrl').focus(); return; }
  $('addUrl').value = '';
  $('url').value = url;
  S.clipNote = false;
  show('main');
  runProbe(true);
}
$('btnAdd').addEventListener('click', addFromQueue);
$('addUrl').addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); addFromQueue(); } });

$('btnSettings').addEventListener('click', () => { show('settings'); refreshTools(true); });
$('btnBack').addEventListener('click', () => show(activeJobs().length ? 'queue' : 'main'));
$('btnEngineUpdate').addEventListener('click', () => updateEngine());
$('btnAppUpdate').addEventListener('click', () => (S.appUpdate ? installAppUpdate() : checkAppUpdate(true)));
$('btnSetupRetry').addEventListener('click', async () => { if (await runSetup()) afterSetup(); });

$('btnMin').addEventListener('click', () => appWindow.minimize());
$('btnMax').addEventListener('click', () => appWindow.toggleMaximize());
$('btnClose').addEventListener('click', async () => {
  if (activeJobs().length && !window.confirm('다운로드가 진행 중이에요. 종료하면 중단돼요. 종료할까요?')) return;
  for (const j of S.jobs.filter((x) => x.status === 'running')) await invoke('cancel_download', { id: j.id }).catch(() => {});
  appWindow.close();
});

window.addEventListener('focus', () => checkClipboard());
document.addEventListener('dragover', (e) => e.preventDefault());
document.addEventListener('drop', (e) => {
  e.preventDefault();
  const text = (e.dataTransfer.getData('text/uri-list') || e.dataTransfer.getData('text/plain') || '').split('\n')[0].trim();
  if (isUrl(text)) { $('url').value = text; S.clipNote = false; show('main'); runProbe(true); }
});

/* ---------- app updates ---------- */

async function checkAppUpdate(manual) {
  S.appUpdateBusy = !!manual; S.appUpdateErr = false; render();
  try {
    const v = await invoke('app_update_check');
    S.appUpdate = v || false;
  } catch {
    S.appUpdate = null;
    S.appUpdateErr = !!manual;
  } finally {
    S.appUpdateBusy = false; render();
  }
}

async function installAppUpdate() {
  if (activeJobs().length && !window.confirm('다운로드가 진행 중이에요. 업데이트하면 중단돼요. 계속할까요?')) return;
  S.appUpdateBusy = true; render();
  try {
    await invoke('app_update_install');
  } catch (e) {
    S.appUpdateBusy = false;
    window.alert(`업데이트하지 못했어요: ${e}`);
    render();
  }
}

/* ---------- boot ---------- */

function afterSetup() {
  show('main');
  checkClipboard();
  if (settings.autoUpdate && settings.lastCheck !== today()) updateEngine({ silent: true });
  else refreshTools(false);
  checkAppUpdate(false);
}

async function boot() {
  await listen('job-progress', (e) => onProgress(e.payload));
  await listen('job-done', (e) => onDone(e.payload));
  await listen('tool-progress', (e) => {
    S.toolProgress[e.payload.name] = e.payload;
    if (S.view === 'setup' || S.view === 'settings') render();
  });
  try { S.version = await invoke('app_version'); } catch { /* ignore */ }
  if (!settings.outDir) {
    settings.outDir = await invoke('default_dir').catch(() => '');
    saveSettings();
  }
  S.tools = await invoke('tools_status', { channel: settings.channel, checkLatest: false }).catch(() => []);
  if (!S.tools.length || S.tools.some((t) => !t.installed)) {
    if (!(await runSetup())) return;
  }
  afterSetup();
}

boot();
