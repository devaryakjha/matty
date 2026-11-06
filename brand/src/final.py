import os
from textpath import text_path, glyph_paths
INK="#111114"; WHITE="#FAFAF9"; STONE="#8A8A90"
G={400:"../fonts/geist400.ttf",500:"../fonts/geist500.ttf",700:"../fonts/geist700.ttf"}

def kdot(wt=500, size=54, cx=32, cy=32, dotk=1.0):
    """k. centred optically on (cx,cy). Returns (inner svg using currentColor-less fill placeholder FILL, bbox)"""
    d = glyph_paths(G[wt],"k",size)[0][0]
    _,_,b = text_path(G[wt],"k",size)
    x0,y0,x1,y1 = b
    stem = {400:.082,500:.098,700:.13}[wt]*size
    dot = stem*1.22*dotk; gap = stem*0.55
    total = (x1-x0)+gap+dot
    tx = cx-total/2-x0; ty = cy-(y0+y1)/2
    return (f'<g transform="translate({tx:.2f} {ty:.2f})"><path fill="FILL" d="{d}"/>'
            f'<rect x="{x1+gap:.2f}" y="{y1-dot:.2f}" width="{dot:.2f}" height="{dot:.2f}" fill="FILL"/></g>')

def svg(inner, vb="0 0 64 64", extra=""):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}"{extra}>{inner}</svg>'

def mark(fg, wt=500): return svg(kdot(wt).replace("FILL",fg))
def avatar(fg,bg,wt=500,size=44,rx=0):
    return svg(f'<rect width="64" height="64" rx="{rx}" fill="{bg}"/>'+kdot(wt,size).replace("FILL",fg))

def favicon_svg():
    inner = kdot(700,50).replace('fill="FILL"','class="m"')
    style = ('<style>.b{fill:#111114}.m{fill:#FAFAF9}'
             '@media (prefers-color-scheme:dark){.b{fill:#FAFAF9}.m{fill:#111114}}</style>')
    return svg(style+'<rect class="b" width="64" height="64" rx="14"/>'+inner)

def wordmark(fg, sub, style="caps", size=18, x=0, y=0):
    if style=="caps":
        tr=size*0.22
        d,w,_=text_path(G[400],"KARYA",size,x=x,y=y,tracking=tr)
        d2,w2,_=text_path(G[400],"LABS",size,x=x+w+size*0.55,y=y,tracking=tr)
        return f'<path fill="{fg}" d="{d}"/><path fill="{sub}" d="{d2}"/>', w+size*0.55+w2-tr
    d,w,_=text_path(G[500],"karya",size,x=x,y=y,tracking=-size*0.01)
    d2,w2,_=text_path(G[500],"labs",size,x=x+w+size*0.24,y=y,tracking=-size*0.01)
    return f'<path fill="{fg}" d="{d}"/><path fill="{sub}" d="{d2}"/>', w+size*0.24+w2

def lockup(fg, sub, style="caps"):
    s=.62; size = 18 if style=="caps" else 24
    t,w = wordmark(fg,sub,style,size,x=64*s+(14 if style=="caps" else 10), y=32*s+size*0.36)
    W = 64*s+(14 if style=="caps" else 10)+w+2
    return svg(f'<g transform="scale({s})">{kdot(500).replace("FILL",fg)}</g>{t}', vb=f"0 0 {W:.1f} {64*s:.1f}")

def social(fg,bg,sub):
    W,H=1280,640
    m = kdot(500,54).replace("FILL",fg)
    t,w = wordmark(fg,sub,"caps",30,x=0,y=0)
    tag_d,tw,_ = text_path(G[400],"Open-source native apps and the tools behind them",26)
    return svg(f'<rect width="{W}" height="{H}" fill="{bg}"/>'
               f'<g transform="translate({W/2-120} {H/2-190}) scale(3.75)">{m}</g>'
               f'<g transform="translate({W/2-w/2:.1f} {H/2+130})">{t}</g>'
               f'<g transform="translate({W/2-tw/2:.1f} {H/2+190})"><path fill="{sub}" d="{tag_d}"/></g>',
               vb=f"0 0 {W} {H}")

def build(out):
    for d in ["mark","avatar","favicon","lockup","social"]: os.makedirs(f"{out}/{d}",exist_ok=True)
    w=lambda p,s: open(f"{out}/{p}","w").write(s)
    w("mark/karya-mark-ink.svg",mark(INK)); w("mark/karya-mark-white.svg",mark(WHITE))
    w("mark/karya-mark-ink-small.svg",mark(INK,700)); w("mark/karya-mark-white-small.svg",mark(WHITE,700))
    w("avatar/karya-avatar-dark.svg",avatar(WHITE,INK)); w("avatar/karya-avatar-light.svg",avatar(INK,WHITE))
    w("favicon/favicon.svg",favicon_svg())
    w("favicon/_fav-dark.svg",avatar(WHITE,INK,700,50,14)); w("favicon/_touch.svg",avatar(WHITE,INK,500,40,0))
    w("lockup/karya-lockup-caps-ink.svg",lockup(INK,STONE)); w("lockup/karya-lockup-caps-white.svg",lockup(WHITE,STONE))
    w("lockup/karya-lockup-lower-ink.svg",lockup(INK,STONE,"lower")); w("lockup/karya-lockup-lower-white.svg",lockup(WHITE,STONE,"lower"))
    w("social/karya-social-dark.svg",social(WHITE,INK,STONE)); w("social/karya-social-light.svg",social(INK,WHITE,STONE))
if __name__=="__main__":
    import sys; build(sys.argv[1])
