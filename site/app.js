/* VOLUMA page: reveal on scroll, bars grow when seen, and the NTFS story swaps frames as you read.
   Everything is progressive enhancement; the page reads fully without it. */
(() => {
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const io = (cb, opts) => 'IntersectionObserver' in window ? new IntersectionObserver(cb, opts) : null;

  // Reveal: hierarchy. Content appears as it enters, once.
  const rv = [...document.querySelectorAll('.rv')];
  const seen = io((es, o) => es.forEach(e => { if (e.isIntersecting) { e.target.classList.add('in'); o.unobserve(e.target); } }), { rootMargin: '0px 0px -8% 0px', threshold: .12 });
  if (seen && !reduce) rv.forEach(el => seen.observe(el)); else rv.forEach(el => el.classList.add('in'));

  // Benchmark bars grow when their chart is on screen: shows the measured ratio, not decoration.
  const charts = [...document.querySelectorAll('.chart, .kpi')];
  const grow = io((es, o) => es.forEach(e => { if (e.isIntersecting) { e.target.classList.add('in'); o.unobserve(e.target); } }), { threshold: .25 });
  if (grow && !reduce) charts.forEach(c => grow.observe(c)); else charts.forEach(c => c.classList.add('in'));

  // NTFS story: the screenshot follows the step being read (state transition).
  const steps = [...document.querySelectorAll('.step')], frames = [...document.querySelectorAll('.frame')];
  if (steps.length && frames.length && 'IntersectionObserver' in window && matchMedia('(min-width: 901px)').matches) {
    const pick = io(es => es.forEach(e => {
      if (!e.isIntersecting) return;
      const n = +e.target.dataset.frame;
      steps.forEach((s, i) => s.classList.toggle('on', i === n));
      frames.forEach((f, i) => f.classList.toggle('on', i === n));
    }), { rootMargin: '-45% 0px -45% 0px' });
    steps.forEach(s => pick.observe(s));
  } else if (frames.length) {
    document.documentElement.classList.add('stack-frames');
    frames.forEach(f => f.classList.add('on'));
  }

  // Copy button for the install command. Feedback for a user action; hidden when the clipboard API is missing.
  const btns = [...document.querySelectorAll('.copy')];
  if (!navigator.clipboard) { btns.forEach(b => b.style.display = 'none'); }
  btns.forEach(b => b.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(b.dataset.copy); const t = b.innerHTML; b.classList.add('done'); b.lastChild.textContent = ' Copied'; setTimeout(() => { b.classList.remove('done'); b.innerHTML = t; }, 1600); } catch (e) { /* user can still select the text */ }
  }));
})();
