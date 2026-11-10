import sys,re
for f in sys.argv[1:]:
    rows=[]
    for l in open(f):
        if l.startswith('GPU_STATS'):
            d=dict(re.findall(r'(\w+)=([\d.]+)',l.split('|')[0]))
            rows.append({k:float(v) for k,v in d.items()})
    mid=rows[5:-3]
    fr=sum(r['frames'] for r in mid); n=len(mid)
    keys=['draws','passes','blur_passes','instances','damaged_mpx','geometry','layer_repaints']
    print(f"{f.split('/')[-1]}: {n} s, {fr/n:.1f} frames/s; per frame: "+", ".join(f"{k} {sum(r[k] for r in mid)/fr:.2f}" for k in keys))
