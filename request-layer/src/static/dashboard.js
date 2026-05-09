/* ============================================================
   ARIA Dashboard — Application Logic
   ============================================================ */
const BASE = window.ARIA_BASE || window.location.origin;
const POLL_MS = 2000;

/* ---- STATE ---- */
let prevData = null;
let failCount = 0;
let uptimeStart = Date.now();
let volumeChart = null;
let donutChart = null;
const sparkCharts = {};
const agentHistory = {};
const eventLog = [];
let activePreset = 'balanced';

/* ---- UTILITIES ---- */
function shortName(n){ return n.replace(/^a2a-/,''); }
function agentStatus(q, s){
  if(q>20||s>4) return 'critical';
  if(q>=5||s>=2) return 'pressure';
  return 'healthy';
}
function dotColor(q,s){
  const st=agentStatus(q,s);
  return st==='critical'?'red':st==='pressure'?'yellow':'green';
}
function fmtUptime(ms){
  const s=Math.floor(ms/1000); const h=Math.floor(s/3600);
  const m=Math.floor((s%3600)/60); const sec=s%60;
  return `${h}h ${String(m).padStart(2,'0')}m ${String(sec).padStart(2,'0')}s`;
}
function now(){ return new Date().toLocaleTimeString('en-US',{hour12:false}); }

/* ---- ANIMATED COUNTER ---- */
const counters = {};
function animateValue(el, target, duration=600){
  if(!el) return;
  const id = el.id || el.dataset.cid || Math.random().toString();
  if(counters[id]) cancelAnimationFrame(counters[id]);
  const start = parseFloat(el.dataset.current||'0');
  const startTime = performance.now();
  const isFloat = String(target).includes('.');
  function step(t){
    const p = Math.min((t-startTime)/duration,1);
    const ease = 1-Math.pow(1-p,3);
    const v = start + (target-start)*ease;
    el.textContent = isFloat ? v.toFixed(1) : Math.round(v);
    el.dataset.current = isFloat ? v.toFixed(1) : String(Math.round(v));
    if(p<1) counters[id] = requestAnimationFrame(step);
  }
  counters[id] = requestAnimationFrame(step);
}

/* ---- CLOCK ---- */
function tickClock(){
  const el=document.getElementById('topbar-time');
  if(el) el.textContent = new Date().toLocaleString('en-US',{hour12:false, hour:'2-digit',minute:'2-digit',second:'2-digit', year:'numeric',month:'short',day:'numeric'});
  const up=document.getElementById('sidebar-uptime');
  if(up) up.textContent = fmtUptime(Date.now()-uptimeStart);
}
setInterval(tickClock, 1000);

/* ---- NAV ---- */
function navigateTo(section){
  document.querySelectorAll('.sidebar-nav a').forEach(a=>a.classList.remove('active'));
  event.currentTarget.classList.add('active');
  const targets = {
    'overview': 'section-kpi',
    'analytics': 'section-charts',
    'configuration': 'section-controls'
  };
  const el = document.getElementById(targets[section]);
  if(el) el.scrollIntoView({behavior:'smooth', block:'start'});
}

/* ---- INIT CHARTS ---- */
function initCharts(){
  const ctx1 = document.getElementById('volume-canvas');
  if(!ctx1) return;
  volumeChart = new Chart(ctx1, {
    type:'line',
    data:{ labels: Array.from({length:30},(_,i)=> (i-29)*2+'s'), datasets:[] },
    options:{
      responsive:true, maintainAspectRatio:false,
      plugins:{ legend:{ labels:{ color:'#8888aa', font:{family:'Inter',size:11}, usePointStyle:true, pointStyle:'circle', boxWidth:8 } } },
      scales:{
        x:{ grid:{color:'rgba(42,42,58,0.4)'}, ticks:{color:'#8888aa',font:{size:10}, maxTicksLimit:10} },
        y:{ beginAtZero:true, grid:{color:'rgba(42,42,58,0.4)'}, ticks:{color:'#8888aa',font:{size:10}} }
      },
      animation:{duration:500},
      elements:{ point:{radius:0,hoverRadius:4}, line:{tension:0.4,borderWidth:2} }
    }
  });

  const ctx2 = document.getElementById('donut-canvas');
  if(!ctx2) return;
  donutChart = new Chart(ctx2, {
    type:'doughnut',
    data:{
      labels:['Exact Cache','Semantic Cache','Cache Misses'],
      datasets:[{ data:[0,0,0], backgroundColor:['#6c63ff','#00d4aa','#2a2a3a'], borderWidth:0, hoverBorderWidth:2, hoverBorderColor:'#fff' }]
    },
    options:{
      responsive:true, cutout:'72%',
      plugins:{ legend:{ position:'bottom', labels:{ color:'#8888aa', font:{family:'Inter',size:11}, usePointStyle:true, padding:14, boxWidth:8 } } },
      animation:{duration:500}
    }
  });
}

/* ---- CHART COLORS ---- */
const CHART_COLORS = ['#6c63ff','#00d4aa','#f59e0b','#ef4444','#ec4899','#3b82f6'];
let colorIdx = 0;
const agentColorMap = {};
function getAgentColor(name){
  if(!agentColorMap[name]) agentColorMap[name]=CHART_COLORS[colorIdx++ % CHART_COLORS.length];
  return agentColorMap[name];
}

/* ---- UPDATE VOLUME CHART ---- */
function updateVolumeChart(agents, data){
  if(!volumeChart) return;
  agents.forEach(name=>{
    const sn = shortName(name);
    if(!agentHistory[name]) agentHistory[name]=Array(30).fill(0);

    // Calculate requests delta from prev poll
    let reqCount = 0;
    if(prevData){
      const prevQ = (prevData.traffic.queue_lengths[name]||0);
      const curQ = (data.traffic.queue_lengths[name]||0);
      const slope = Math.abs(data.traffic.velocity_slopes[name]||0);
      reqCount = Math.max(0, curQ - prevQ) + (slope > 0 ? Math.ceil(slope) : 0);
    }
    agentHistory[name].push(reqCount);
    if(agentHistory[name].length>30) agentHistory[name].shift();

    const c = getAgentColor(name);
    // Find existing dataset by matching shortName
    const existing = volumeChart.data.datasets.find(d=>d.label===sn);
    if(!existing){
      volumeChart.data.datasets.push({
        label: sn, data:[...agentHistory[name]],
        borderColor:c, backgroundColor:c+'22', fill:true
      });
    } else {
      existing.data = [...agentHistory[name]];
    }
  });
  volumeChart.update('none'); // no animation flicker
}

/* ---- UPDATE DONUT ---- */
function updateDonut(cache){
  if(!donutChart) return;
  const exact = cache.exact_hits || 0;
  const semantic = cache.semantic_hits || 0;
  const misses = cache.misses || 0;
  const total = exact + semantic + misses;

  if(total === 0){
    // Show empty state
    donutChart.data.datasets[0].data = [0, 0, 1];
    donutChart.data.datasets[0].backgroundColor = ['#6c63ff','#00d4aa','#1a1a24'];
  } else {
    donutChart.data.datasets[0].data = [exact, semantic, misses];
    donutChart.data.datasets[0].backgroundColor = ['#6c63ff','#00d4aa','#2a2a3a'];
  }
  donutChart.update();
  const el=document.getElementById('donut-pct');
  if(el) el.textContent = cache.hit_rate_percent.toFixed(1)+'%';
}

/* ---- SPARKLINES ---- */
function updateSparkline(agent, status, queueLen){
  const canvas = document.getElementById('spark-'+agent);
  if(!canvas) return;
  const key = '_sparkhist_'+agent;
  if(!window[key]) window[key]=Array(20).fill(0);
  window[key].push(queueLen || Math.round(Math.random()*2));
  if(window[key].length>20) window[key].shift();
  const color = status==='critical'?'#ef4444':status==='pressure'?'#f59e0b':'#10b981';
  if(sparkCharts[agent]){
    sparkCharts[agent].data.datasets[0].data = [...window[key]];
    sparkCharts[agent].data.datasets[0].borderColor = color;
    sparkCharts[agent].data.datasets[0].backgroundColor = color+'22';
    sparkCharts[agent].update('none');
  } else {
    sparkCharts[agent] = new Chart(canvas, {
      type:'line',
      data:{ labels:Array(20).fill(''), datasets:[{ data:[...window[key]], borderColor:color, backgroundColor:color+'22', fill:true, borderWidth:1.5, pointRadius:0, tension:0.4 }] },
      options:{ responsive:true, maintainAspectRatio:false, plugins:{legend:{display:false}}, scales:{x:{display:false},y:{display:false,beginAtZero:true}}, animation:{duration:300} }
    });
  }
}

/* ---- SIDEBAR AGENTS ---- */
function updateSidebarAgents(agents, data){
  const container = document.getElementById('sidebar-agents');
  if(!container) return;
  container.innerHTML = '';
  agents.forEach(name=>{
    const q = data.traffic.queue_lengths[name]||0;
    const s = Math.abs(data.traffic.velocity_slopes[name]||0);
    const dc = dotColor(q,s);
    container.innerHTML += `<div class="agent-item"><span class="agent-dot ${dc}"></span><span>${shortName(name)}</span><span class="agent-badge">${q}</span></div>`;
  });
}

/* ---- KPI CARDS ---- */
function updateKPIs(data){
  const cache = data.cache;
  const total = cache.exact_hits + cache.semantic_hits + cache.misses;
  const saved = cache.exact_hits + cache.semantic_hits;

  const hrEl = document.getElementById('kpi-hitrate');
  if(hrEl){
    animateValue(hrEl, cache.hit_rate_percent);
    hrEl.className = 'kpi-value ' + (cache.hit_rate_percent>60?'green':cache.hit_rate_percent>30?'amber':'red');
  }
  if(prevData){
    const diff = cache.hit_rate_percent - prevData.cache.hit_rate_percent;
    setTrend('trend-hitrate', diff, Math.abs(diff).toFixed(1)+'% from last poll');
  }

  const tsEl = document.getElementById('kpi-served');
  if(tsEl) animateValue(tsEl, total);
  if(prevData){
    const prevTotal = prevData.cache.exact_hits+prevData.cache.semantic_hits+prevData.cache.misses;
    const rps = ((total-prevTotal)/2).toFixed(1);
    setTrend('trend-served', parseFloat(rps), rps+' req/s');
  }

  const csEl = document.getElementById('kpi-saved');
  if(csEl) animateValue(csEl, saved);
  const pct = total>0? ((saved/total)*100).toFixed(0) : 0;
  setTrend('trend-saved', 1, pct+'% of total', true);

  const opEl = document.getElementById('kpi-overloads');
  if(opEl) animateValue(opEl, data.traffic.prevented_overloads||0);

  const stEl = document.getElementById('sidebar-total');
  if(stEl) stEl.textContent = total.toLocaleString();
}

function setTrend(id, val, text, forceUp){
  const el = document.getElementById(id);
  if(!el) return;
  const cls = forceUp||val>0?'up':val<0?'down':'neutral';
  el.className = 'kpi-trend '+cls;
  el.innerHTML = `<span>${val>0||forceUp?'▲':val<0?'▼':'—'}</span> <span>${text}</span>`;
}

/* ---- AGENT CARDS ---- */
function renderAgentCards(agents, data){
  const container = document.getElementById('agents-container');
  if(!container) return;

  // Remove cards for agents no longer present
  const currentIds = new Set(agents.map(n=>'acard-'+n));
  Array.from(container.children).forEach(c=>{
    if(!currentIds.has(c.id)) c.remove();
  });

  agents.forEach(name=>{
    const q = data.traffic.queue_lengths[name]||0;
    const slope = data.traffic.velocity_slopes[name]||0;
    const limit = data.rate_limits[name]||10;
    const status = agentStatus(q, Math.abs(slope));
    let card = document.getElementById('acard-'+name);
    if(!card){
      card = document.createElement('div');
      card.id = 'acard-'+name;
      card.className = 'agent-card';
      container.appendChild(card);
    }

    let trendHtml, trendCls;
    if(slope>2){ trendHtml=`Accelerating ↑ (${slope.toFixed(1)})`; trendCls='accel'; }
    else if(slope<-1){ trendHtml=`Calming ↓ (${slope.toFixed(1)})`; trendCls='calming'; }
    else { trendHtml=`Stable → (${slope.toFixed(1)})`; trendCls='stable'; }

    const events = data.traffic.proactive_tightening_events||[];
    const hasTighten = events.some(e=>e.agent===name);
    const tightenBadge = hasTighten?'<span class="tighten-badge">⚡ ARIA tightened limit</span>':'';
    const qPct = Math.min(q/50*100,100);
    const qColor = q>25?'var(--danger)':q>10?'var(--warning)':'var(--success)';

    card.innerHTML = `
      <div class="agent-card-header">
        <span class="agent-card-name">${shortName(name)}</span>
        <span class="status-badge ${status}">${status}</span>
      </div>
      <div class="agent-metrics">
        <div class="agent-metric"><div class="m-label">Queue</div><div class="m-value">${q}</div><div class="queue-bar"><div class="queue-bar-fill" style="width:${qPct}%;background:${qColor}"></div></div></div>
        <div class="agent-metric"><div class="m-label">Rate Limit</div><div class="m-value">${limit}/s</div></div>
        <div class="agent-metric"><div class="m-label">Velocity</div><div class="m-value" style="color:${slope>2?'var(--danger)':slope<-1?'var(--secondary)':'var(--success)'}">${slope>0?'+':''}${slope.toFixed(1)}</div></div>
      </div>
      <div class="sparkline-wrap"><canvas id="spark-${name}" height="60"></canvas></div>
      <div class="trend-line ${trendCls}">${trendHtml}</div>
      ${tightenBadge}
      <div class="agent-controls">
        <input type="number" id="rl-input-${name}" value="${Math.round(limit)}" min="1" max="100" title="Rate limit">
        <button class="btn-sm" onclick="setLimit('${name}')">Set</button>
        <button class="btn-sm" onclick="clearCache('${name}')">Clear Cache</button>
        <button class="btn-sm" onclick="testAgent('${name}')">Test</button>
      </div>`;
    updateSparkline(name, status, q);
  });
}

/* ---- INSIGHTS ---- */
function generateInsights(data){
  const insights = [];
  const cache = data.cache;
  const total = cache.exact_hits+cache.semantic_hits+cache.misses;
  if(cache.semantic_hits>cache.exact_hits && cache.semantic_hits>0)
    insights.push({sev:'green',title:'Semantic cache is your hero',desc:`Users asking same questions in different ways. Semantic matching caught ${cache.semantic_hits} similar queries.`});
  if(cache.hit_rate_percent>70)
    insights.push({sev:'green',title:'Excellent cache efficiency',desc:`ARIA serving ${cache.hit_rate_percent.toFixed(1)}% from cache. Agent compute usage is minimal.`});
  if(cache.hit_rate_percent>0 && cache.hit_rate_percent<30)
    insights.push({sev:'amber',title:'Cache warming needed',desc:'Low hit rate — mostly unique queries. Consider lowering similarity threshold.'});
  (data.agents||[]).forEach(name=>{
    const q=data.traffic.queue_lengths[name]||0;
    const s=data.traffic.velocity_slopes[name]||0;
    if(q>10) insights.push({sev:'amber',title:`Queue pressure on ${shortName(name)}`,desc:`Queue at ${q} requests. ARIA absorbing the spike.`});
    if(Math.abs(s)>3) insights.push({sev:'red',title:`Traffic spike on ${shortName(name)}`,desc:`Velocity at ${s.toFixed(1)}x. ARIA proactively tightened limits.`});
  });
  if((data.traffic.prevented_overloads||0)>0)
    insights.push({sev:'green',title:`${data.traffic.prevented_overloads} overloads prevented`,desc:`ARIA's predictive limiting stopped potential cascading failures.`});
  const agents=data.agents||[];
  const allHealthy = agents.length>0 && agents.every(n=>agentStatus(data.traffic.queue_lengths[n]||0,Math.abs(data.traffic.velocity_slopes[n]||0))==='healthy');
  if(allHealthy && cache.hit_rate_percent>=60)
    insights.push({sev:'green',title:'System operating optimally',desc:'All agents healthy, cache performing well.'});
  if(insights.length===0)
    insights.push({sev:'green',title:'Monitoring active',desc:'No significant events. System stable.'});
  const container=document.getElementById('insights-container');
  if(container) container.innerHTML = insights.map(i=>`<div class="insight-card sev-${i.sev}"><div class="insight-title">${i.title}</div><div class="insight-desc">${i.desc}</div></div>`).join('');
}

/* ---- EVENTS LOG ---- */
function updateEventLog(data){
  if(!prevData) return;
  const nc = data.cache, pc = prevData.cache;
  const exactDiff = nc.exact_hits - pc.exact_hits;
  const semDiff = nc.semantic_hits - pc.semantic_hits;
  const missDiff = nc.misses - pc.misses;
  for(let i=0;i<Math.min(exactDiff,3);i++) addEvent('#6c63ff','Exact cache hit served');
  for(let i=0;i<Math.min(semDiff,3);i++) addEvent('#00d4aa','Semantic match served');
  if(missDiff>0) addEvent('#8888aa',`${missDiff} request${missDiff>1?'s':''} forwarded to agent`);
  (data.agents||[]).forEach(name=>{
    const pq=(prevData.traffic.queue_lengths[name]||0);
    const cq=(data.traffic.queue_lengths[name]||0);
    if(cq>pq) addEvent('#f59e0b',`Request queued for ${shortName(name)} (pos ${cq})`);
  });
  const ne = (data.traffic.proactive_tightening_events||[]).length;
  const pe = (prevData.traffic.proactive_tightening_events||[]).length;
  if(ne>pe){
    const latest = data.traffic.proactive_tightening_events[ne-1];
    addEvent('#10b981',`Proactive tightening on ${shortName(latest.agent)}`);
  }
  renderEvents();
}
function addEvent(color, text){
  eventLog.unshift({time:now(),color,text});
  if(eventLog.length>50) eventLog.pop();
}
function renderEvents(){
  const container=document.getElementById('events-container');
  if(!container) return;
  if(eventLog.length===0){
    container.innerHTML='<div class="event-item"><span class="event-dot" style="background:var(--primary)"></span><span class="event-time"></span><span class="event-text">Waiting for events...</span></div>';
    return;
  }
  container.innerHTML = eventLog.map(e=>`<div class="event-item"><span class="event-dot" style="background:${e.color}"></span><span class="event-time">${e.time}</span><span class="event-text">${e.text}</span></div>`).join('');
}

/* ---- API ACTIONS ---- */
async function setLimit(agent){
  const input = document.getElementById('rl-input-'+agent);
  if(!input) return;
  const val = parseInt(input.value);
  if(isNaN(val)||val<1) return;
  await fetch(`${BASE}/config/rate-limit`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent_name:agent,limit:val})});
  addEvent('#3b82f6',`Rate limit for ${shortName(agent)} → ${val}/s`);
  renderEvents();
}
async function clearCache(agent){
  await fetch(`${BASE}/cache/${agent}`,{method:'DELETE'});
  addEvent('#8888aa',`Cache cleared for ${shortName(agent)}`);
  renderEvents();
}
async function testAgent(agent){
  const r = await fetch(`${BASE}/proxy/${agent}/health`,{method:'POST'});
  const d = await r.json();
  addEvent(d.reachable?'#10b981':'#ef4444', `${shortName(agent)}: ${d.reachable?'reachable ✓':'unreachable ✗'}`);
  renderEvents();
}
async function applyPreset(level){
  const limits = {conservative:5, balanced:10, aggressive:20};
  const val = limits[level];
  activePreset = level;
  document.querySelectorAll('.btn-preset').forEach(b=>b.classList.toggle('active',b.dataset.preset===level));
  const agents = prevData?.agents || [];
  for(const name of agents)
    await fetch(`${BASE}/config/rate-limit`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({agent_name:name,limit:val})});
  addEvent('#6c63ff',`Preset "${level}" applied — all agents → ${val}/s`);
  renderEvents();
}
async function clearAllCaches(){
  const btn=document.getElementById('btn-clear-all');
  if(btn) btn.textContent='Clearing...';
  const agents = prevData?.agents || [];
  for(const name of agents) await fetch(`${BASE}/cache/${name}`,{method:'DELETE'});
  if(btn) btn.textContent='✓ Cleared';
  setTimeout(()=>{ if(btn) btn.textContent='Clear All Caches'; },2000);
  addEvent('#8888aa','All caches cleared');
  renderEvents();
}

/* ---- DEMO SIMULATION ---- */
async function runDemo(){
  const btn = document.getElementById('btn-demo');
  if(btn){ btn.textContent='Running...'; btn.disabled=true; }
  addEvent('#6c63ff','🚀 Demo simulation started');
  renderEvents();
  try { await fetch(`${BASE}/demo/start`,{method:'POST'}); }
  catch(e){ console.error('Demo start error:',e); }
  setTimeout(()=>{
    if(btn){ btn.textContent='▶ Run Demo'; btn.disabled=false; }
  },12000);
}

/* ---- MAIN POLL ---- */
async function poll(){
  try{
    const r = await fetch(`${BASE}/stats`);
    if(!r.ok) throw new Error(r.statusText);
    const data = await r.json();
    failCount = 0;
    document.getElementById('error-banner')?.classList.remove('visible');

    const agents = data.agents || [];
    if(data.uptime_seconds) uptimeStart = Date.now() - data.uptime_seconds*1000;

    updateKPIs(data);
    updateVolumeChart(agents, data);
    updateDonut(data.cache);
    updateSidebarAgents(agents, data);
    renderAgentCards(agents, data);
    updateEventLog(data);
    if(!window._lastInsight || Date.now()-window._lastInsight>10000){
      generateInsights(data);
      window._lastInsight = Date.now();
    }
    prevData = data;
  }catch(e){
    failCount++;
    console.error('Poll error:',e);
    if(failCount>=3) document.getElementById('error-banner')?.classList.add('visible');
  }
}

/* ---- BOOT ---- */
window.addEventListener('DOMContentLoaded',()=>{
  tickClock();
  initCharts();
  poll();
  setInterval(poll, POLL_MS);
});
