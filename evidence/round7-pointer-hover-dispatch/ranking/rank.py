from pathlib import Path
from collections import Counter,defaultdict
import gzip,json,re
d=Path.home()/'Library/Caches/lightningpanda/round7-local/census-1f1be0324'
out=Path(__file__).resolve().parent
areas={'html','dom','url','urlpattern','FileAPI','IndexedDB','streams','encoding','xhr','webmessaging','workers','service-workers','fetch','webstorage','webaudio','webcodecs','webnn','svg','mathml','intersection-observer','resize-observer','performance-timeline','user-timing','navigation-timing','resource-timing','hr-time','longtask-timing','screen-orientation','mediacapture-streams','mediacapture-transform','pointerevents','uievents','selection','geometry','webvtt','mediastream-recording','media-source','media-capabilities','media-playback-quality','image-capture','fullscreen','canvas','offscreen-canvas','web-animations','custom-elements','shadow-dom','worklets','web-locks','file-system-access','fs','storage','compression','contacts','geolocation','notifications','clipboard-apis','scheduler','web-share','payment-request','idle-detection','webtransport','video-rvfc','eventsource','beacon','server-timing'}
block=re.compile(r'(?i)crash|regression|security|sandbox|document-isolation-policy|fetch/metadata|policy[-/]|content[-_]security|trusted[-_]types|integrity|origin-agent|private-network|secure.context|authentication|reporting[-/]|cors|cross[-_]site|leak|privileg|cross-origin|crossorigin|xorigin|insecure|opaque|taint|csp|credential|cookie|permission|mixed.content|overflow|out.of.bounds|malformed|detached|detach[-/]|wasm|fuzz|exploit|use.after|referrer-policy|editing|css/')
areas.update({'navigation-api','long-animation-frame','soft-navigation-heuristics','preload','is-input-pending','largest-contentful-paint','webxr','scroll-to-text-fragment','screen-wake-lock','paint-timing','pointerlock','scroll-animations','webrtc-svc','imagebitmap-renderingcontext','screen-details','picture-in-picture','page-visibility','vibration','speculation-rules','websockets','network-information','mediasession','keyboard-map','element-timing','domparsing','performance-measure-memory','resize-observer','gamepad','deviceorientation'})
stats=defaultdict(Counter); groups=defaultdict(Counter); kinds=Counter(); versions={}; pages=[]
for line in gzip.open(d/'evidence/pages.jsonl.gz','rt'):
 u,lp,ch,pr,src,hold=json.loads(line)
 if not lp or not ch or hold or u.split('/')[0] not in areas or block.search(u):continue
 if ch[:2]!=['complete','OK'] or lp[:2]==['complete','OK']:continue
 key='/'.join(u.split('/')[:2]) if u.count('/')>1 else u.split('/')[0]
 for counts in [stats[u.split('/')[0]],groups[key]]:
  counts['pages']+=1;counts['lpNames']+=lp[3];counts['chromeNames']+=ch[3];counts['net']+=ch[3]-lp[3];counts[lp[0]+'/'+str(lp[1])]+=1
 pages.append((u,lp,ch,src))
for coll in [stats,groups]:
 for k,v in sorted(coll.items(),key=lambda i:-i[1]['net'])[:25]:print(k,dict(v))
 print()
(out/'rank.json').write_text(json.dumps({'areas':stats,'groups':groups,'rows':pages},indent=1)+'\n')

named=defaultdict(Counter); chosen={r[0] for r in pages}
for line in gzip.open(d/'evidence/named.jsonl.gz','rt'):
 u,n,occ,lp,ch,old=json.loads(line)
 if u not in chosen: continue
 if ch and ch[0]=='Pass' and not(lp and lp[0]=='Pass'): named[u]['chromeOnly']+=1
 if lp and lp[0]=='Pass' and not(ch and ch[0]=='Pass'): named[u]['lpOnly']+=1
byshard=defaultdict(dict)
for u,lp,ch,keys in pages: byshard[keys[0]][u]=lp
causes=defaultdict(lambda:{'pages':0,'chromeOnly':0,'lpOnly':0,'examples':[]})
for shard,selected in byshard.items():
 path=d/'raw'/f'{shard}.json'
 if not path.exists(): continue
 for r in json.loads(path.read_text())['results']:
  u=r['url']
  if u not in selected:continue
  error=r.get('error') or ''
  if error and block.search(error):continue
  prefix=u.split('/')[0]
  if not error:
   cause='No reported harness error message / '+prefix
  elif re.search(r'(?:ReferenceError: )?Can.t find variable: \w+',error):
   cause='Missing binding '+re.search(r'Can.t find variable: (\w+)',error)[1]
  else:
   cause=re.sub(r'\bhttps?://[^ ]+|[0-9a-f]{8}-[a-f0-9-]{27,}', '<url>',error.splitlines()[0])[:220]
  row=causes[cause];row['pages']+=1
  for key in ('chromeOnly','lpOnly'):row[key]+=named[u][key]
  if len(row['examples'])<3: row['examples'].append(u)
ranking=sorted(causes.items(),key=lambda pair:(-pair[1]['chromeOnly'],-pair[1]['pages'],pair[0]))
(out/'causes.json').write_text(json.dumps(ranking,indent=1)+'\n')
print('eligible rows',len(chosen),'raw cause rows',sum(x['pages'] for x in causes.values()))
for k,v in ranking[:14]: print(k[:90],v['pages'],v['chromeOnly'])
