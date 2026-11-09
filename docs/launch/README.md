# Launch kit

Drafts for the VOLUMA launch. Edit the voice to taste, but keep the numbers: every one comes from `tests/bench` and is on https://festomanolo.com/voluma/#benchmarks.

## Before you post (blockers, in order)
1. Push `main` and the `v0.1.0` tag, confirm the release page and `pip install nolo-voluma` work from a clean machine.
2. Deploy the website bundle (`voluma-website-*.zip`) so the links and screenshots resolve.
3. The demo GIF is recorded: `site/assets/img/demo.gif` (rebuild it with `site/make_demo_gif.py` from a frame capture). It is in the README and the NTFS guide; attach it to every post.
4. Notarise the app if you can. Until then, say plainly that it is not notarised (the posts below do).
5. Have a second machine ready to answer "does it work on Apple Silicon?" with a real result, or keep saying it is untested.

## Order and timing
| Day | Where | File |
|---|---|---|
| 1 (Tue to Thu, 8 to 10am US Eastern) | Hacker News | `show-hn.md` |
| 1, a few hours later | r/rust | `reddit.md` (r/rust) |
| 2 | r/macapps, r/MacOS, r/DataHoarder | `reddit.md` |
| 3 | Product Hunt (12:01am Pacific) | `product-hunt.md` |
| 3 | X, Mastodon, LinkedIn | `social.md` |
| Week 1 | Blog post, then submit to dev.to, Lobsters, This Week in Rust | `blog-post.md` |
| Week 1 | AlternativeTo, awesome-macOS, awesome-mac, awesome-rust pull requests | `directories.md` |

Do not post everywhere in one hour. You want to be able to answer every comment for the first few hours.

## Ground rules
- Lead with what it does and the evidence. Put the limits in the first comment, not buried.
- Never claim "safe". Say what was tested and against what.
- Read each subreddit's self-promotion rules first. Some want an established account or a weekly thread.
- Reply to criticism with facts, thank people for bug reports, and open an issue for each real one.
