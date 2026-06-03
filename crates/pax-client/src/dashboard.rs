//! Embedded web control panel for the client. Served at `GET /`. Pure HTML + vanilla JS
//! so it works on any headless/VPS/RDP machine — no graphics stack required.

pub const HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Pax Americana — Client</title>
<style>
  :root{
    --bg:#0a0e16;--panel:#101723;--elev:#172130;--border:#222d40;
    --text:#e9f0fb;--dim:#909fb7;--faint:#5a6880;
    --ember:#ff7849;--green:#34d399;--red:#fb5d6e;--amber:#f5b454;--info:#53b9f2;
  }
  *{box-sizing:border-box}
  body{margin:0;background:var(--bg);color:var(--text);
    font-family:ui-sans-serif,Segoe UI,Roboto,Helvetica,Arial,sans-serif;font-size:14px}
  .wrap{max-width:1000px;margin:0 auto;padding:20px}
  header{display:flex;align-items:center;gap:12px;padding:6px 0 16px}
  .logo{color:var(--ember);font-size:22px}.title{font-weight:700;letter-spacing:.5px;font-size:18px}
  .badge{background:#33261f;border:1px solid #c9572f;color:var(--ember);border-radius:6px;padding:2px 8px;font-size:11px;font-weight:700}
  .pill{margin-left:auto;display:inline-flex;align-items:center;gap:7px;background:var(--elev);border:1px solid var(--border);border-radius:20px;padding:5px 12px;font-weight:600}
  .grid{display:grid;grid-template-columns:repeat(3,1fr);gap:10px;margin-bottom:14px}
  .card{background:var(--panel);border:1px solid var(--border);border-radius:12px;padding:14px;margin-bottom:14px}
  .tile .k{color:var(--faint);font-size:10.5px;font-weight:700;text-transform:uppercase}
  .tile .v{font-size:22px;font-weight:700;margin-top:4px}
  .sec-title{color:var(--dim);font-size:11.5px;font-weight:700;text-transform:uppercase;margin-bottom:10px}
  .sec-title:before{content:"▍";color:var(--ember);margin-right:6px}
  label{color:var(--dim);font-size:12px;display:block;margin-bottom:4px}
  input,select{background:#0d1420;border:1px solid var(--border);color:var(--text);border-radius:8px;padding:8px 10px;font-size:14px;width:100%}
  input[type=range]{padding:0}
  .row{display:flex;gap:12px;flex-wrap:wrap;align-items:end;margin-bottom:10px}
  .row>div{flex:1;min-width:120px}
  button{border:0;border-radius:8px;padding:10px 18px;font-weight:700;font-size:14px;cursor:pointer;color:#000}
  button:hover{filter:brightness(1.08)}
  .b-go{background:var(--green)}.b-stop{background:var(--red)}.b-warn{background:var(--amber)}.b-ember{background:var(--ember)}
  .hint{color:var(--faint);font-size:11px;margin-top:4px}
  .log{background:#0d1420;border:1px solid var(--border);border-radius:10px;padding:10px;height:200px;overflow:auto;font-family:ui-monospace,Consolas,monospace;font-size:12.5px}
  .seg{display:inline-flex;background:#0d1420;border:1px solid var(--border);border-radius:9px;padding:3px;gap:3px}
  .seg button{background:transparent;color:var(--dim);padding:6px 12px;font-size:13px}
  .seg button.on{background:var(--border);color:#fff}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <span class="logo">⬢</span><span class="title">PAX AMERICANA</span><span class="badge">CLIENT</span>
    <span class="pill"><span id="dot">●</span><span id="connText">connecting…</span></span>
  </header>

  <div class="grid">
    <div class="card tile"><div class="k">Net Liquidation</div><div class="v" id="balance" style="color:var(--green)">—</div></div>
    <div class="card tile"><div class="k">Master Balance</div><div class="v" id="mbalance" style="color:var(--info)">—</div></div>
    <div class="card tile"><div class="k">Positions (M · C)</div><div class="v" id="posCount" style="color:var(--amber)">—</div></div>
  </div>

  <div class="card">
    <div class="sec-title">Engine</div>
    <div class="row">
      <div style="flex:0"><button class="b-go" id="startBtn" onclick="ctl('start')">▶ START</button></div>
      <div style="flex:0"><button class="b-stop" onclick="ctl('stop')">■ STOP</button></div>
      <div style="flex:0"><button class="b-warn" onclick="if(confirm('Cancel orders and flatten ALL client positions?'))ctl('close_all')">CLOSE ALL</button></div>
      <div></div>
      <div style="flex:0"><label>API key</label><input id="apiKey" placeholder="optional" style="width:160px"/></div>
    </div>
  </div>

  <div class="card">
    <div class="sec-title">Settings</div>
    <div class="row">
      <div><label>Account</label><div class="seg" id="account"></div></div>
      <div><label>Trading</label><div class="seg" id="trade"></div></div>
      <div><label>Execution</label><div class="seg" id="exec"></div></div>
    </div>
    <div class="row">
      <div><label>Size Multiplier (<span id="multV">1.0</span>×)</label><input type="range" id="mult" min="0.1" max="5" step="0.1"/></div>
      <div><label>Max Drawdown (<span id="ddV">10</span>%)</label><input type="range" id="dd" min="1" max="50" step="0.5"/></div>
    </div>
    <div class="row">
      <div><label>Max Position $ (0=off)</label><input type="number" id="maxNotional"/></div>
      <div><label>Max Position Qty (0=off)</label><input type="number" id="maxQty"/></div>
    </div>
    <div class="row">
      <div><label>IB Host</label><input id="host"/></div>
      <div><label>Live port</label><input type="number" id="portLive"/></div>
      <div><label>Paper port</label><input type="number" id="portPaper"/></div>
    </div>
    <div class="row">
      <div style="flex:0"><button class="b-ember" onclick="saveSettings()">SAVE SETTINGS</button></div>
      <div class="hint">Multiplier / drawdown / modes apply live. Host &amp; ports apply on the next START.</div>
    </div>
  </div>

  <div class="card"><div class="sec-title">Live Order Feed</div><div class="log" id="log"></div></div>
</div>

<script>
const $=id=>document.getElementById(id);
function hdrs(){const k=$("apiKey").value.trim();localStorage.setItem("paxKey",k);return k?{"X-API-Key":k}:{}}
async function getJSON(p){const r=await fetch(p,{headers:hdrs()});if(!r.ok)throw new Error(r.status);return r.json()}
async function post(p,b){const r=await fetch(p,{method:"POST",headers:Object.assign({"Content-Type":"application/json"},hdrs()),body:JSON.stringify(b)});if(!r.ok)alert(p+" failed: "+r.status);return r.ok}
const money=v=>(v<0?"-":"")+"$"+Math.abs(v).toLocaleString(undefined,{minimumFractionDigits:2,maximumFractionDigits:2});
const colorFor=l=>({OK:"var(--green)",WARN:"var(--amber)",ERR:"var(--red)",INFO:"var(--info)",BUY:"var(--green)",SELL:"var(--red)"}[l]||"var(--text)");

function seg(el,opts,val,onpick){el.innerHTML="";for(const[v,lab]of opts){const b=document.createElement("button");b.textContent=lab;if(v===val)b.className="on";b.onclick=()=>onpick(v);el.appendChild(b)}}
let loaded=false,cur={};
function buildSegs(c){
  seg($("account"),[["live","Live"],["paper","Paper"]],c.account_mode,v=>{cur.account_mode=v;saveSettings()});
  seg($("trade"),[["long_short","Long & Short"],["long_only","Long Only"]],c.trade_mode,v=>{cur.trade_mode=v;saveSettings()});
  seg($("exec"),[["existing","Existing + New"],["new","New Only"]],c.execution_mode,v=>{cur.execution_mode=v;saveSettings()});
}
$("mult").oninput=()=>$("multV").textContent=(+$("mult").value).toFixed(1);
$("dd").oninput=()=>$("ddV").textContent=(+$("dd").value).toFixed(1);

async function ctl(action){await post("/control",{action})}
async function saveSettings(){
  const b={account_mode:cur.account_mode,trade_mode:cur.trade_mode,execution_mode:cur.execution_mode,
    multiplier:+$("mult").value,max_drawdown_pct:+$("dd").value,
    max_position_notional:+$("maxNotional").value,max_position_qty:+$("maxQty").value,
    ib_host:$("host").value.trim(),ib_port_live:+$("portLive").value,ib_port_paper:+$("portPaper").value};
  await post("/settings",b);
}
async function tick(){
  try{
    const s=await getJSON("/state");
    const halt=s.drawdown_hit, on=s.connected;
    $("dot").style.color=halt?"var(--amber)":(on?"var(--green)":"var(--red)");
    $("connText").textContent=halt?"Drawdown halt":(on?"Connected — syncing":(s.running?"Connecting…":"Stopped"));
    $("balance").textContent=money(s.client_balance||0);
    $("mbalance").textContent=money(s.master_balance||0);
    $("posCount").textContent=(s.master_positions||0)+" · "+(s.client_positions||0);
    $("startBtn").textContent=s.running?"▶ RUNNING":"▶ START";
    const c=s.controls||{}; cur=Object.assign({},c);
    if(!loaded){
      $("mult").value=c.multiplier; $("multV").textContent=(+c.multiplier).toFixed(1);
      $("dd").value=c.max_drawdown_pct; $("ddV").textContent=(+c.max_drawdown_pct).toFixed(1);
      $("maxNotional").value=c.max_position_notional; $("maxQty").value=c.max_position_qty;
      $("host").value=c.ib_host; $("portLive").value=c.ib_port_live; $("portPaper").value=c.ib_port_paper;
      loaded=true;
    }
    buildSegs(c);
  }catch(e){$("connText").textContent="panel unreachable";$("dot").style.color="var(--red)"}
  try{const lines=await getJSON("/log");const el=$("log");
    el.innerHTML=lines.map(l=>`<div style="color:${colorFor(l.level)}">[${l.ts}] ${l.msg}</div>`).join("");el.scrollTop=el.scrollHeight;
  }catch(e){}
}
$("apiKey").value=localStorage.getItem("paxKey")||"";
tick();setInterval(tick,1500);
</script>
</body>
</html>"##;
