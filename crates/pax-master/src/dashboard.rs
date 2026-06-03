//! Embedded web control panel for the master. Served at `GET /`. Pure HTML + vanilla JS
//! (no build step, no graphics stack) so it works on any headless/VPS/RDP machine.

pub const HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Pax Americana — Master</title>
<style>
  :root{
    --bg:#0a0e16; --panel:#101723; --elev:#172130; --border:#222d40;
    --text:#e9f0fb; --dim:#909fb7; --faint:#5a6880;
    --ember:#ff7849; --green:#34d399; --red:#fb5d6e; --amber:#f5b454; --info:#53b9f2;
  }
  *{box-sizing:border-box}
  body{margin:0;background:var(--bg);color:var(--text);
    font-family:ui-sans-serif,Segoe UI,Roboto,Helvetica,Arial,sans-serif;font-size:14px}
  .wrap{max-width:1000px;margin:0 auto;padding:20px}
  header{display:flex;align-items:center;gap:12px;padding:6px 0 16px}
  .logo{color:var(--ember);font-size:22px}
  .title{font-weight:700;letter-spacing:.5px;font-size:18px}
  .badge{background:#33261f;border:1px solid #c9572f;color:var(--ember);
    border-radius:6px;padding:2px 8px;font-size:11px;font-weight:700}
  .pill{margin-left:auto;display:inline-flex;align-items:center;gap:7px;background:var(--elev);
    border:1px solid var(--border);border-radius:20px;padding:5px 12px;font-weight:600}
  .dot{font-size:11px}
  .grid{display:grid;grid-template-columns:repeat(4,1fr);gap:10px;margin-bottom:14px}
  .card{background:var(--panel);border:1px solid var(--border);border-radius:12px;padding:14px}
  .tile .k{color:var(--faint);font-size:10.5px;font-weight:700;text-transform:uppercase}
  .tile .v{font-size:22px;font-weight:700;margin-top:4px}
  .sec-title{color:var(--dim);font-size:11.5px;font-weight:700;text-transform:uppercase;margin-bottom:10px}
  .sec-title:before{content:"▍";color:var(--ember);margin-right:6px}
  label{color:var(--dim);font-size:12px;display:block;margin-bottom:4px}
  input,select{background:#0d1420;border:1px solid var(--border);color:var(--text);
    border-radius:8px;padding:8px 10px;font-size:14px;width:100%}
  .row{display:flex;gap:12px;flex-wrap:wrap;align-items:end;margin-bottom:8px}
  .row>div{flex:1;min-width:120px}
  button{background:var(--ember);color:#000;border:0;border-radius:8px;padding:10px 18px;
    font-weight:700;font-size:14px;cursor:pointer}
  button:hover{filter:brightness(1.08)}
  table{width:100%;border-collapse:collapse;font-variant-numeric:tabular-nums}
  th{color:var(--faint);text-align:left;font-size:10.5px;text-transform:uppercase;padding:4px 8px}
  td{padding:4px 8px;border-top:1px solid var(--border);font-family:ui-monospace,Consolas,monospace}
  .log{background:#0d1420;border:1px solid var(--border);border-radius:10px;padding:10px;
    height:220px;overflow:auto;font-family:ui-monospace,Consolas,monospace;font-size:12.5px}
  .log div{white-space:pre-wrap}
  .muted{color:var(--faint);font-size:11px;margin-top:6px}
  .hint{color:var(--faint);font-size:11px}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <span class="logo">⬢</span><span class="title">PAX AMERICANA</span>
    <span class="badge">MASTER</span>
    <span class="pill"><span class="dot" id="dot">●</span><span id="connText">connecting…</span></span>
  </header>

  <div class="grid">
    <div class="card tile"><div class="k">Account</div><div class="v" id="account">—</div></div>
    <div class="card tile"><div class="k">Net Liquidation</div><div class="v" id="balance" style="color:var(--green)">—</div></div>
    <div class="card tile"><div class="k">Positions</div><div class="v" id="posCount" style="color:var(--info)">—</div></div>
    <div class="card tile"><div class="k">Working Orders</div><div class="v" id="woCount" style="color:var(--amber)">—</div></div>
  </div>

  <div class="card" style="margin-bottom:14px">
    <div class="sec-title">Connection</div>
    <div class="row">
      <div><label>IB Host</label><input id="host" value="127.0.0.1"/></div>
      <div><label>Live port</label><input id="portLive" type="number" value="7496"/></div>
      <div><label>Paper port</label><input id="portPaper" type="number" value="7497"/></div>
      <div><label>Mode</label><select id="mode"><option value="paper">Paper</option><option value="live">Live</option></select></div>
    </div>
    <div class="row">
      <div style="flex:0"><button onclick="applyConfig()">APPLY &amp; RECONNECT</button></div>
      <div><label>API key (only if the master sets one)</label><input id="apiKey" placeholder="optional"/></div>
    </div>
    <div class="muted" id="endpoint"></div>
    <div class="hint">TWS: 7496 live / 7497 paper&nbsp;·&nbsp;IB Gateway: 4001 / 4002</div>
  </div>

  <div class="card" style="margin-bottom:14px">
    <div class="sec-title">Broadcast Structure</div>
    <table><thead><tr><th>Symbol</th><th>Net Qty</th><th>Side</th><th>Avg Cost</th></tr></thead>
    <tbody id="positions"></tbody></table>
  </div>

  <div class="card">
    <div class="sec-title">Event Log</div>
    <div class="log" id="log"></div>
  </div>
</div>

<script>
const $ = id => document.getElementById(id);
function key(){ return $("apiKey").value.trim(); }
function hdrs(){ const k=key(); localStorage.setItem("paxKey",k); return k?{"X-API-Key":k}:{}; }
async function getJSON(p){ const r=await fetch(p,{headers:hdrs()}); if(!r.ok) throw new Error(r.status); return r.json(); }
const fmtMoney = v => (v<0?"-":"")+"$"+Math.abs(v).toLocaleString(undefined,{minimumFractionDigits:2,maximumFractionDigits:2});
const colorFor = lvl => ({OK:"var(--green)",WARN:"var(--amber)",ERR:"var(--red)",INFO:"var(--info)"}[lvl]||"var(--text)");

let cfgLoaded=false;
async function loadConfig(){
  try{ const c=await getJSON("/config");
    if(!cfgLoaded){ $("host").value=c.host; $("portLive").value=c.port_live; $("portPaper").value=c.port_paper; $("mode").value=c.mode; cfgLoaded=true; }
  }catch(e){}
}
async function applyConfig(){
  const body={host:$("host").value.trim(),port_live:+$("portLive").value,port_paper:+$("portPaper").value,mode:$("mode").value};
  try{ const r=await fetch("/config",{method:"POST",headers:Object.assign({"Content-Type":"application/json"},hdrs()),body:JSON.stringify(body)});
    if(!r.ok){ alert("Apply failed: "+r.status); return; } }
  catch(e){ alert("Apply failed: "+e); }
}
async function tick(){
  try{
    const s=await getJSON("/snapshot");
    $("dot").style.color = s.connected?"var(--green)":"var(--red)";
    $("connText").textContent = s.connected?"Connected — broadcasting":"Disconnected";
    $("account").textContent = s.account||"—";
    $("balance").textContent = fmtMoney(s.balance||0);
    $("posCount").textContent = (s.positions||[]).length;
    $("woCount").textContent = (s.working_orders||[]).length;
    const tb=$("positions"); tb.innerHTML="";
    if(!(s.positions||[]).length){ tb.innerHTML='<tr><td class="hint" colspan=4>flat — no open positions</td></tr>'; }
    for(const p of s.positions||[]){
      const long=p.net_qty>=0;
      tb.insertAdjacentHTML("beforeend",
        `<tr><td style="color:#fff">${p.symbol}</td><td>${p.net_qty.toFixed(0)}</td>`+
        `<td style="color:${long?'var(--green)':'var(--red)'}">${long?'LONG':'SHORT'}</td>`+
        `<td style="color:var(--dim)">${(p.avg_cost||0).toFixed(2)}</td></tr>`);
    }
  }catch(e){ $("connText").textContent="master unreachable"; $("dot").style.color="var(--red)"; }
  try{
    const lines=await getJSON("/log"); const el=$("log");
    el.innerHTML=lines.map(l=>`<div style="color:${colorFor(l.level)}">[${l.ts}] ${l.msg}</div>`).join("");
    el.scrollTop=el.scrollHeight;
  }catch(e){}
  loadConfig();
}
$("apiKey").value = localStorage.getItem("paxKey")||"";
tick(); setInterval(tick,1500);
</script>
</body>
</html>"##;
