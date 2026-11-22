from pathlib import Path
import json,os,subprocess,sys
w=Path(__file__).resolve().parent;c=w.parent;ref=c/'round6-reference-158/census'
if __name__ == '__main__':
 label,binary=sys.argv[1:]
 assert label in ('base','candidate')
 cmd=['bun','bench/wpt/run-no-native-retry.mjs','--wpt',str(c/'wptci/wpt-full'),'--kind','testharness','--dirs','non-css','--jobs','1','--server','wptserve','--testdriver','--http2','--generated-globals',str(c/'round6-inventory/MANIFEST-pinned.json'),'--manifest-inventory','--measure-reviewed-skips','--tests',str(w/'urls.json'),'--bin',label+'='+str(binary),'--out',str(w/(label+'.json'))]
 env=dict(os.environ,WPT_CHROME_STATE_DIR=str(w/'chrome-state'),WPT_CHROME_REFERENCE_MODE='wpt-stable');env.pop('WPT_CHROME_PROFILE',None)
 with (w/(label+'.log')).open('w') as out:subprocess.run(cmd,cwd=ref/'runner',env=env,check=True,stdout=out,stderr=subprocess.STDOUT)
 d=json.load(open(w/(label+'.json')));assert not d['restarts'] and not d['watchdogKills'] and {x['url'] for x in d['results']}==set(json.load(open(w/'urls.json')))
