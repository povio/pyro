"""Validate the candidate against SVG2 and Filter Effects independently of Chrome quirks."""
import json
from pathlib import Path
w=Path(__file__).parent
base,head,chrome=(json.loads((w/(label+'-probe.json')).read_text()) for label in ('base','candidate','chrome158'))
assert len(base)==len(head)==len(chrome)==8
assert all(not row['supported'] for row in base)
for h,c in zip(head,chrome):
    assert (h['tag'],h['attr'])==(c['tag'],c['attr'])
    assert h['brands']==c['brands']==['SVGAnimatedNumberList','SVGNumberList']
    assert h['identity']==c['identity']==[True]*4
    assert h['initial']==[[1,20,-3.5],[1,20,-3.5],True,True]
    assert h['sync']==[4,4,True,True]
    assert h['insertion']==[True,True,True,[4,4,5,7]]
    assert h['indexed']==c['indexed']==[True,7,[7,4,5,7]]
    assert h['removal']==[True,[7,5,7],[7,5,7]]
    assert h['crossList']==[True,True,11,12]
    assert h['clearAbsent']==h['removeLast']==''
    assert h['cleared']==[0,0,'']
    assert h['namespace']==c['namespace']==[7,5,7,12]
    assert h['errors']==c['errors']==dict(readonly='NoModificationAllowedError',readonlyItem='NoModificationAllowedError',badArg='TypeError',badIndex='IndexSizeError',nan='TypeError')
    p=dict(h['parses'])
    for invalid in ('1,2x','1,,2','1,','1e999','','2px','3%'):
        assert p[invalid]==[], (h['attr'],invalid)
    assert p['10 20']==[10,20] and p['1e-2,+3.5']==[0.009999999776482582,3.5]
    assert p['1\f2']==[1,2]
    if h['attr']=='rotate':
        assert p['1-2']==[1,-2] and p['1.2.3']==[1.2000000476837158,0.30000001192092896] and p['1/**/2']==[1,2]
    else:
        assert p['1-2']==p['1.2.3']==p['1/**/2']==[]
print('Eight element classes passed list, item, parsing, namespace, identity and exception controls.')
