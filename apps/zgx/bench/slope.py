import sys
for f in sys.argv[1:]:
    rows=[]
    for l in open(f):
        p=l.split()
        if len(p)==2 and ':' in p[1]:
            m,s=p[1].split(':'); rows.append((float(p[0]), int(m)*60+float(s)))
    t_end=rows[-1][0]-1.0  # quits a second after the last frame
    win=[r for r in rows if t_end-40<=r[0]<=t_end-2]
    a,b=win[0],win[-1]
    print(f"{f.split('/')[-1]}: {len(rows)} samples, outside CPU over {b[0]-a[0]:.1f}s mid-stream: {100*(b[1]-a[1])/(b[0]-a[0]):.1f}%")
