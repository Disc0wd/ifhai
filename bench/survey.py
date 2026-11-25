import json, random, subprocess, re, sys, os, concurrent.futures as cf, datetime
S=os.environ.get('S','.')  # working directory holding feed.json, heads/ and results.json
feed=json.load(open(S+'/research/feed.json'))['items']
repos=[]
for i in feed:
    for l in (i.get('links') or []):
        if l.get('type')=='GitHub' and re.fullmatch(r'[\w.-]+/[\w.-]+', l.get('url') or ''):
            repos.append(l['url']); break
random.seed(7); sample=random.sample(sorted(set(repos)),150)
X86=('x86_64','amd64','x64'); OTHER=('aarch64','arm64','armhf','armv7','i386','i686','x86_32')
def one(repo):
    r={'repo':repo}
    p=subprocess.run(['gh','api',f'repos/{repo}/releases/latest'],capture_output=True,text=True,timeout=60)
    if p.returncode!=0:
        r['status']='no_release' if '404' in p.stderr else 'api_error'; return r
    j=json.loads(p.stdout); r['tag']=j['tag_name']; r['published']=j.get('published_at')
    ai=[a for a in j['assets'] if a['name'].lower().endswith('.appimage')]
    if not ai: r['status']='no_appimage'; return r
    tagged=[a for a in ai if any(t in a['name'].lower() for t in X86)]
    untagged=[a for a in ai if not any(t in a['name'].lower() for t in X86+OTHER)]
    pool=tagged or untagged
    if not pool: r['status']='no_x86_64'; return r
    r['status']='ok'; r['candidates']=len(pool); r['digest']=any(a.get('digest') for a in pool)
    sums=[a for a in j['assets'] if re.search(r'sha256|checksum',a['name'],re.I)]
    r['sidecar']=bool(sums)
    if len(pool)==1:
        a=pool[0]; r['size']=a['size']; fn=S+'/survey/heads/'+re.sub(r'[^\w.-]','_',repo)
        c=subprocess.run(['curl','-fsSL','--proto','=https','-r','0-2097151','-o',fn,a['browser_download_url']],capture_output=True,timeout=120)
        r['head']=fn if c.returncode==0 else None
    return r
with cf.ThreadPoolExecutor(8) as ex: res=list(ex.map(one,sample))
json.dump(res,open(S+'/survey/results.json','w'))
from collections import Counter
print(Counter(r['status'] for r in res))
