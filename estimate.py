"""Distinct estimated strict PAGES per brief and union, from census 5892e136 (non-CSS).
Strict page = Chrome strict pass and LP not strict. Estimates only; nothing measured."""
import gzip,json,re,sys,collections
B='/Users/xban1x/Library/Caches/lightningpanda/round7-queue/ev/evidence/rc5-census-516dc3159-combined/'
SEL={
 'Q01':[r'^(?!.*command-and-commandfor)'],  # filled from q01 list below
 'Q03':[r'^html/rendering/bidi-rendering/unicode-bidi-ua-rules\.html$',r'^html/rendering/non-replaced-elements/form-controls/resets\.html$'],
 'Q04':[r'^svg/styling/presentation-attributes-(relevant|unknown|irrelevant|special-cases)\.html$'],
 'Q05':[r'^fetch/data-urls/processing\.any\.(html|worker\.html|sharedworker\.html)$'],
 'Q06':[r'^html/semantics/forms/the-input-element/type-change-state\.html$'],
 'Q07':[r'^html/semantics/embedded-content/image-maps/image-map-processing-model/hash-name-reference\.html$',r'^html/semantics/embedded-content/the-area-element/area-coords\.html$'],
 'Q08':[r'^custom-elements/form-associated/ElementInternals-setFormValue\.html$'],
 'Q09':['__COUNT2__'],  # Studio: frozen mspace fix covers 2 pages (IDs not reported)
 'Q02':[r'^quirks/unitless-length/quirks\.html$'],
 'Q10':[],
 'Q11':[r'^svg/path/property/'],
 'Q12':[r'^html/canvas/(element|offscreen)/(manual/)?wide-gamut-canvas/'],
 'C01':[r'^html/canvas/(element|offscreen)/fill-and-stroke-styles/2d\.(fillStyle|strokeStyle|gradient)\.(parse|relativecolor|colormix|object\.invalidcolor)'],
 'C02':[r'^html/canvas/(element|offscreen)/shadows/'],
 'C03':[r'^html/canvas/(element|offscreen)/compositing/2d\.composite\.(.*\.clear|uncovered\.nocontext)'],
 'C04':[r'^html/canvas/(element|offscreen)/text/2d\.text\.(font\.parse|fontVariantCaps)'],
 'C05':[r'^html/canvas/(element|offscreen)/text/2d\.text\.(measure|draw\.(align|baseline|space)|lang)'],
 'P01':[r'^svg/idlharness\.window\.html$'],
 'P02':[r'^html/browsers/history/the-history-interface/00[12]\.html$'],
 'P03':[r'^workers/modules/(dedicated|shared)-worker-(options-credentials|import-referrer|import-blob-url)'],
 'P04':[r'^html/semantics/forms/the-input-element/show-picker-(disabled-readonly|user-gesture)\.html$'],
 'P05':[],
 'P06':[r'^scheduler/tentative/yield/'],
 'P07':[r'^fetch/content-encoding/zstd/(zstd-body|big-zstd-body|zstd-navigation)'],
 'P08':[r'^html/syntax/parsing/html5lib_'],
 'P09':[r'^custom-elements/reactions/AriaMixin-element-attributes\.html$'],
 'P10':[r'^dom/nodes/Document-createElement-namespace\.html$'],
 'P11':[r'^webidl/ecmascript-binding/'],
 'P12':[r'^html/browsers/the-window-object/window-properties\.https\.html$'],
 'P13':[r'^fetch/api/request/destination/fetch-destination'],
 'P14':[r'^svg/animations/'],
 'E01':[r'^encoding/textdecoder-arguments\.any\.(worker\.)?html$'],
 'OWN-body-textstream':[r'^fetch/api/body/textstream'],
 'OWN-xhr-send-usp':[r'^xhr/send-usp\.any\.(worker\.)?html$'],
 'P15':[r'^intersection-observer/v2/'],
 'F01':[r'^(referrer-policy|content-security-policy)/gen/.*/sharedworker-(module|import|import-data)\.',r'^workers/modules/shared-worker'],
 'F02':[r'^html/interaction/focus/focusgroup/'],
 'F03':[r'^html/semantics/forms/the-select-element/customizable-select'],
 'F04':[r'^html/semantics/scripting-1/the-script-element/module/'],
 'F05':[r'^referrer-policy/gen/.*/worker-(classic|module)\.'],
 'F06':[r'^content-security-policy/gen/.*/worker-import-data\.'],
 'F07':[r'^html/semantics/scripting-1/the-script-element/(json-module|css-module)/'],
 'F08':[r'^html/semantics/scripting-1/the-script-element/moving-between-documents/'],
 'F09':[r'^html/semantics/scripting-1/the-script-element/execution-timing/'],
 'F10':[r'^html/syntax/speculative-parsing/generated/'],
 'OWN-blob-textstream':[r'^FileAPI/.*[Tt]ext[Ss]tream'],
}
MACHINE_DIRS={'MacBook':('html/','fetch/','IndexedDB/','encoding/'),'Studio':('css/','referrer-policy/','content-security-policy/')}
def load():
  strict=set()
  for l in gzip.open(B+'comparison/pages.jsonl.gz','rt'):
    r=json.loads(l)
    if r[2][2] and not r[1][2]: strict.add(r[0])
  return strict
def main(ledger,out):
  strict=load()
  q01=[u for _,u,_ in json.load(open('/Users/xban1x/Library/Caches/lightningpanda/round7-queue/q01-named-access-pages.json')) if 'command-and-commandfor' not in u]
  L=[json.loads(l) for l in open(ledger)]
  per={};
  for r in L:
    if r['kind']!='brief' and r['id'] not in SEL: continue
    i=r['id']
    if i=='Q01': pages=set(q01)&strict
    elif SEL.get(i)==['__COUNT2__']: pages={'mathml/relations/css-styling/<page-1 of Q09 mspace>','mathml/relations/css-styling/<page-2 of Q09 mspace>'}
    elif i in SEL: pages={u for u in strict if any(re.search(p,u) for p in SEL[i])}
    else: pages=None  # SC/S rows: CSS corpus, not in this census
    per[i]=(r['state'],pages,r)
  tiers={'implemented':('frozen','implemented-unbuilt','landed'),'in_progress':('claimed','active','reserved'),'queued':('validated','proposed')}
  res={'basis':'census 5892e136 strict pages (Chrome strict pass, LP not strict); ESTIMATES, nothing measured','tiers':{}}
  for t,states in tiers.items():
    u=set(); css=0; byd=collections.Counter()
    for i,(st,pg,r) in per.items():
      if st not in states: continue
      if pg is None:
        sp=r.get('receipts',{}).get('strict_page_gains_potential') or r.get('receipts',{}).get('pages') or 0
        try: css+=int(str(sp).split()[0])
        except: pass
      else: u|=pg
    for p in u: byd['/'.join(p.split('/')[:2]) if p.split('/')[0] in('html','fetch','svg','dom') else p.split('/')[0]]+=1
    res['tiers'][t]={'distinct_census_pages':len(u),'of_which_tentative':sum(1 for x in u if 'tentative' in x),'studio_css_reported_pages_not_deduped':css,'by_dir':dict(byd.most_common())}
  res['per_brief']={i:{'state':st,'pages':(len(pg) if pg is not None else None),'tentative':(sum(1 for x in pg if 'tentative' in x) if pg is not None else None)} for i,(st,pg,r) in per.items()}
  cum=res['tiers']['implemented']['distinct_census_pages']+res['tiers']['implemented']['studio_css_reported_pages_not_deduped']
  res['implemented_total_estimate']=cum
  res['milestones_reached']=[m for m in (250,500,750,1000) if cum>=m]
  json.dump(res,open(out,'w'),indent=1)
  print(json.dumps({k:v for k,v in res.items() if k!='per_brief'},indent=1)[:2500])
  print({i:v for i,v in res['per_brief'].items() if v['pages'] is not None})
main(sys.argv[1],sys.argv[2])
