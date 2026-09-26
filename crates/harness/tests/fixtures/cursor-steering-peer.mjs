// Protocol stress peer: reject overlapping SDK turns, echo exact prompt order.
import readline from 'node:readline';
const first = JSON.parse(process.argv[2]);
const out = value => process.stdout.write(JSON.stringify(value) + '\n');
let busy = false;
let timer;
function turn(prompt, delay) {
  if (busy) {out({ev:'fatal',message:'overlapping sends'}); process.exitCode=1; return;}
  busy=true;
  timer=setTimeout(() => {
    out({ev:'text',text:prompt});
    busy=false;
    out({ev:'turn',status:'finished'});
  }, delay);
}
out({ev:'ready',agentId:'burst-agent',model:'auto'});
turn('INITIAL',first.prompt.includes('cancel') ? 30000 : 30);
const rl=readline.createInterface({input:process.stdin});
rl.on('line',line=>{
  const msg=JSON.parse(line);
  if(msg.op==='interrupt') {clearTimeout(timer);busy=false;out({ev:'turn',status:'cancelled'});}
  else if(msg.op==='steer') {out({ev:'steered'}); if(busy) out({ev:'text',text:msg.prompt}); else turn(msg.prompt,1);}
});
rl.on('close',()=>{clearTimeout(timer);});
