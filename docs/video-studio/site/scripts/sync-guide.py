"""Splits docs/video-studio/USER_GUIDE.md into the site's pages: one page per numbered section,
grouped into folders. The guide stays the single source; run `npm run sync` after editing it
(`npm run deploy` does this first). The home page and src/content/docs/introduction.mdx are
written by hand and left alone."""
import re, sys, pathlib, shutil, subprocess, json
here = pathlib.Path(__file__).resolve().parent.parent
src = (here.parent / 'USER_GUIDE.md').read_text()
out = here / 'src/content/docs'
# section number -> (group, slug, title, order, description)
P = {
 1:("getting-started","before-you-start","Before you start",1,"What to install and set up before your first project."),
 2:("getting-started","quick-start","Quick start",2,"Make your first video, step by step, from a new project to finished files."),
 3:("getting-started","projects","Creating and opening projects",3,"New projects, intents, templates and project files."),
 4:("getting-started","project-window","The project window",4,"Sidebar, spec editor, inspector, toolbar and status bar."),
 5:("write","spec","Writing the spec",1,"Scenes, voice-over, shots, music and the conventions the app understands."),
 6:("write","claude","Working with Claude",2,"Improve the spec with Claude and review every change it proposes."),
 7:("write","media","Media",3,"Add media by reference and relink files that move."),
 8:("configure","brand-presets","Brand presets",1,"Logos, colors, fonts, voice, rules and instruction sets for a brand."),
 9:("configure","outputs","Outputs and run configs",2,"Platform presets, run configs, models and limits."),
 10:("configure","project-inspector","The Project inspector",3,"Intent, finish, models and app recording for one project."),
 16:("configure","settings","Settings reference",4,"Every setting, with its default."),
 11:("run","validation","Validation",1,"Catch problems before a run, at no cost."),
 12:("run","running","Running",2,"Dry runs, production runs, questions and stopping."),
 13:("run","results","Results, revisions and logs",3,"Review outputs, revise a run and export logs."),
 14:("run","recording-apps","Recording apps during a run",4,"Let Claude drive and record an app for footage."),
 15:("run","voice-and-music","Voice-over and music",5,"ElevenLabs or Cartesia, choosing a voice, delivery tags and music."),
 17:("reference","shortcuts","Keyboard shortcuts",1,"Every keyboard shortcut."),
 18:("reference","storage","Where things are stored",2,"Projects, settings, keys and run folders."),
 19:("reference","troubleshooting","Troubleshooting",3,"Common problems and what to do."),
}
GROUPS = {"getting-started":("Getting started",1),"write":("Write the video",2),"configure":("Configure",3),"run":("Make the video",4),"reference":("Reference",5)}
parts = re.split(r'^## (\d+)\. .*$', src, flags=re.M)
intro = parts[0]
secs = {int(parts[i]): parts[i+1] for i in range(1,len(parts),2)}
# anchor map: "#7-media" -> page; also sub-headings
def slugify(t): return re.sub(r'[^a-z0-9 -]','',t.lower()).replace(' ','-')
amap = {}
for n,body in secs.items():
    g,s,t,_,_ = P[n]
    amap[f"{n}-{slugify(t if n!=0 else t)}"] = f"/{g}/{s}/"
    for h in re.findall(r'^###+ (.*)$', body, flags=re.M):
        amap.setdefault(slugify(h), f"/{g}/{s}/#{slugify(h)}")
# fix numbered anchors from original headings
for m in re.finditer(r'^## (\d+)\. (.*)$', src, flags=re.M):
    n=int(m.group(1)); g,s,*_=P[n]; amap[f"{n}-{slugify(m.group(2))}"]=f"/{g}/{s}/"
def fixlinks(b):
    def r(m):
        a=m.group(2)
        if a in amap: return f"]({amap[a]})"
        print("UNMAPPED", a, file=sys.stderr); return m.group(0)
    b=re.sub(r'\]\((#)([^)]+)\)', r, b)
    b=re.sub(r'\[README\.md › The CLI\]\(README\.md#the-cli\)', 'the developer README (`docs/video-studio/README.md`, *The CLI*)', b)
    b=re.sub(r'\[README\.md\]\(README\.md\)', 'the developer README (`docs/video-studio/README.md`)', b)
    b=b.replace('(README › Signing)','(see *Signing* in the developer README)')
    return b
def images(b):
    """Pictures: copied, at most 1600 px wide, into public/guide/, and linked to themselves so a click opens them full size."""
    return re.sub(r'!\[([^\]]*)\]\(images/([^)]+)\)', lambda m: f"[![{m.group(1)}](/guide/{m.group(2)})](/guide/{m.group(2)})", b)
def copy_images():
    src = here.parent / "images"
    dst = here / "public" / "guide"
    if dst.exists(): shutil.rmtree(dst)
    dst.mkdir(parents=True)
    for f in sorted(src.glob("*.png")):
        # sips (macOS) makes them 1600 px wide at most; elsewhere they are copied as they are.
        if shutil.which("sips"):
            subprocess.run(["sips", "-Z", "1600", str(f), "--out", str(dst / f.name)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            shutil.copy(f, dst / f.name)
    print(f"copied {len(list(dst.glob('*.png')))} pictures")
def demote(b):  # ### -> ##, #### -> ###
    return re.sub(r'^(#{3,6}) ', lambda m: '#'*(len(m.group(1))-1)+' ', b, flags=re.M)
for g,(label,order) in GROUPS.items():
    d=out/g
    if d.exists(): shutil.rmtree(d)
    d.mkdir(parents=True)
for n,body in secs.items():
    g,s,t,o,desc = P[n]
    body = body.strip().rstrip('-').strip()
    body = re.sub(r'```mermaid.*?```\n*', '', body, flags=re.S)
    body = demote(images(fixlinks(body)))
    (out/g/f"{s}.md").write_text(f"---\ntitle: {json.dumps(t)}\ndescription: {json.dumps(desc)}\nsidebar:\n  order: {o}\n---\n\n{body}\n")
# group index pages
for g,(label,order) in GROUPS.items():
    pages=[(P[n][3],P[n][1],P[n][2],P[n][4]) for n in P if P[n][0]==g]
    lst="\n".join(f"- [{t}](/{g}/{s}/): {d}" for _,s,t,d in sorted(pages))
    (out/g/"index.md").write_text(f"---\ntitle: {label}\nsidebar:\n  order: {order}\n  group:\n    label: {label}\n---\n\n{lst}\n")
copy_images()
print(f"wrote {len(secs)} pages from USER_GUIDE.md")
