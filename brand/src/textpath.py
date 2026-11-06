import uharfbuzz as hb
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.boundsPen import BoundsPen
import functools

@functools.lru_cache(None)
def load(path):
    data = open(path,'rb').read()
    return TTFont(path), hb.Font(hb.Face(data))

def text_path(path, text, size, x=0, y=0, tracking=0, features=None):
    """Return (svg d, width, bounds) for text set at baseline y, origin x."""
    tt, font = load(path)
    upem = tt['head'].unitsPerEm
    buf = hb.Buffer(); buf.add_str(text); buf.guess_segment_properties()
    hb.shape(font, buf, features or {})
    gs = tt.getGlyphSet(); order = tt.getGlyphOrder()
    s = size/upem
    pen = SVGPathPen(gs)
    bp = BoundsPen(gs)
    cx = 0
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        name = order[info.codepoint]
        t = (s,0,0,-s, x+(cx+pos.x_offset)*s, y-pos.y_offset*s)
        gs[name].draw(TransformPen(pen, t))
        gs[name].draw(TransformPen(bp, t))
        cx += pos.x_advance + tracking*upem/size if False else pos.x_advance
        cx += tracking/s
    return pen.getCommands(), cx*s, bp.bounds

def glyph_paths(path, text, size):
    tt, font = load(path)
    upem = tt['head'].unitsPerEm
    buf = hb.Buffer(); buf.add_str(text); buf.guess_segment_properties()
    hb.shape(font, buf, {})
    gs = tt.getGlyphSet(); order = tt.getGlyphOrder(); s=size/upem; cx=0; out=[]
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        pen = SVGPathPen(gs); name=order[info.codepoint]
        gs[name].draw(TransformPen(pen,(s,0,0,-s,(cx+pos.x_offset)*s,-pos.y_offset*s)))
        out.append((pen.getCommands(), name)); cx+=pos.x_advance
    return out
