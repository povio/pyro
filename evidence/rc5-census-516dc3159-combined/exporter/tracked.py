"""Track old loss cohorts without treating an unmeasured row as a current failure."""
from collections import Counter, defaultdict
import json
from reference_index import PREVIOUS, REFERENCE, read, readlines, require, sha

HISTORIC_SHA='7a594d9ce70d2c4bb2590e9d7f5415ece4108cc29cb7ebda925a2364e2388c5a'
CHANGES_SHA='9ebb0fadebc4f525073021e33fc6cba526863544b6861324d51ea517f15bee1e'
AUDIT_SHA='adde6c4da710f89f20b6a2b7d905ec80279538ac48c779bbe599f1a68982bb82'
AUDIT_COMMIT='7589acd4c996d10eec55a375145afff49fb28eae'
EARLIER='8e7144b7e7de68be3b5b345138abe2c1dcc6d232'

def identity(row):
    return row['url'],row['canonical'],row['occurrence']

class Cohorts:
    def __init__(self,prior,audit):
        historical=prior/'historical-292.json';changes=prior/'prior-pass-changes.jsonl.gz'
        require(sha(historical)==HISTORIC_SHA and sha(changes)==CHANGES_SHA,'wrong frozen tracked inputs')
        self.labels={'historical292':read(historical),'previous106':[]}
        old_audit={}
        if audit is not None:
            require(sha(audit)==AUDIT_SHA,'incorrect published 106 audit')
            old_audit={identity(r):r['observedGroup'] for r in read(audit)}
            require(len(old_audit)==106,'incorrect audit cardinality')
        for url,name,occ,direction,previous,stable,earlier,pages in readlines(changes):
            if direction!='loss':continue
            require(earlier[0]=='Pass' and (previous is None or previous[0]!='Pass'),'invalid prior loss')
            key=(url,name,occ)
            require(not old_audit or key in old_audit,'unrecognized prior loss')
            self.labels['previous106'].append(dict(url=url,canonical=name,occurrence=occ,
                round7=previous,chrome158=stable,combined=earlier,round7Page=pages[0],
                chrome158Page=pages[1],combinedPage=pages[2],observedGroup=old_audit.get(key)))
        self.byurl=defaultdict(set)
        for group,expected in [('historical292',292),('previous106',106)]:
            rows=self.labels[group];require(len(rows)==len({identity(x) for x in rows})==expected,'wrong tracked set: '+group)
            for row in rows:self.byurl[row['url']].add((row['canonical'],row['occurrence']))
        self.sources=dict(censusCommit=REFERENCE,previousLP=PREVIOUS,earlierLP=EARLIER,
            historical292SHA256=HISTORIC_SHA,previousPassChangesSHA256=CHANGES_SHA,
            original106AuditCommit=AUDIT_COMMIT if old_audit else None,
            original106AuditSHA256=AUDIT_SHA if old_audit else None)

    def export(self,out,head,observations,covered):
        summary={}
        for group,rows in self.labels.items():
            result=[];counts=Counter()
            for row in rows:
                key=identity(row);url,n,i=key;coverage,shard,reason=covered[url]
                measurement=observations.get(key)
                if coverage in ('measured','runnerStalledPlaceholder'):
                    require(measurement is not None,'missing completed tracked page: '+url)
                    status,page,error=measurement
                    state=('runnerStalledPlaceholder' if coverage=='runnerStalledPlaceholder' else
                           'missingName' if status is None else status[0])
                else:
                    require(measurement is None,'observation on unmeasured page: '+url)
                    state=coverage;status=page=error=None
                counts[state]+=1
                result.append(dict(original=row,currentHead=head,currentState=state,current=status,
                    currentPage=page,currentSubtestDiagnostic=error,coverage=coverage,shard=shard,reason=reason))
            (out/(group+'.json')).write_text(json.dumps(result,indent=1,sort_keys=True)+'\n')
            summary[group]=dict(total=len(result),currentDisposition=counts)
        (out/'historical-tracking.json').write_text(json.dumps(dict(sources=self.sources,summary=summary),indent=1,sort_keys=True)+'\n')
