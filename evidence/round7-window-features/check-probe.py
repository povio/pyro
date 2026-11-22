"""Check standard behavior and the explicitly requested desktop geometry."""
from pathlib import Path
import json
w=Path(__file__).parent
signed,candidate=[json.loads((w/(label+'-probe.json')).read_text()) for label in ['chrome158','candidate']]
for result in (signed,candidate):
    root=result['root']
    assert root['visible']==[True]*6 and root['distinct']==6 and root['brand']=='BarProp' and root['replaceable']
    for row in result['rows']:
        child=row['child']
        assert child and child['brand']=='BarProp' and child['identity'], row
        assert (not row['returnedHandle']) == (row['features'] in ['noopener=1','noreferrer=yes']), row
        assert child['hasOpener']==row['returnedHandle'] and child['hasReferrer']==(row['features']!='noreferrer=yes'),row
        popup=row['features'].startswith('popup=yes,width') or row['features']=='popup=1junk' or row['features'].startswith(('width=','height='))
        assert child['visible']==[not popup]*6,row
        if row['features'].startswith('popup=yes,width'):
            assert child['size']==[310,207] and child['position']==[33,22],row
    assert result['reuse']['openerPrint']
    for entry in result['reuse']['results']:
        assert entry['sameHandle'] and entry['child']['hasReferrer'] and entry['child']['referrer'].endswith('/page')
        assert not entry['child']['print']
    assert result['namedNoOpener']['returnedNull'] and result['namedNoOpener']['child']['name']=='report-window'
    assert result['namedNoOpener']['child']['hasReferrer'] and not result['namedNoOpener']['child']['hasOpener']
    event_rows=result['events']
    assert len(event_rows)==16  # fourteen new geometry/feature cases, one named open, one isolated named open
    assert event_rows[2]['windowFeatures']==['left=33','top=22','width=310','height=207','resizable']
    assert event_rows[10]['windowFeatures']==['width=0','height=0','resizable']
    assert event_rows[11]['windowFeatures']==['width=-404','height=405','resizable']
    assert event_rows[12]['windowFeatures']==['width=405','height=204','resizable']
    assert event_rows[13]['windowFeatures']==['height=204','resizable']
    assert event_rows[-1]['windowName']=='report-window' and 'noopener' in event_rows[-1]['windowFeatures']
assert candidate['frame']==dict(before=[True,True],after=[True,True])
assert signed['frame']==dict(before=[True,True],after=[False,False])
print('Validated fourteen feature cases, six BarProp objects, named reuse with both referrers, fresh target print emulation and detached bars.')
