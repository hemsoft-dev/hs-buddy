"""Measured Electron layout evidence, scoped to the synthetic lifecycle profile."""
import math
import time

ASSISTANT_DEFAULT = 350  # src/appUtils.ts; checked against the real layout consumer.
STARTUP_PHASES = ("electron-seed", "electron-restart")
PHASES = (*STARTUP_PHASES, "electron-installed-rollback")
SCRIPT = """(async()=>{
 const rect=e=>{if(!e)return null;const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}};
 const body=document.querySelector('main.app-body');
 const split=body?.querySelector(':scope > .split-view');
 const container=split?.querySelector(':scope > .split-view-container');
 return {config:await window.ipcRenderer.invoke('config:get-config'),
  viewport:{width:innerWidth,height:innerHeight},outer:{width:outerWidth,height:outerHeight},dpr:devicePixelRatio,
  body:rect(body),activity:rect(body?.querySelector(':scope > .activity-bar')),split:rect(split),
  panes:[...(container?.children||[])].filter(e=>e.classList.contains('split-view-view')).map(rect)};
})()"""


def number(value):
    return type(value) in (int, float) and math.isfinite(value)


def require(condition, message):
    if not condition:
        raise RuntimeError("Unqualified Electron layout: " + message)


def close(left, right):
    require(number(left) and number(right) and abs(left - right) <= 1, "geometry/value mismatch")


def sizes(config, lengths):
    ui = config.get("ui", {})
    value = ui.get("paneSizes")
    require(ui.get("assistantOpen") is False, "only the two-visible-pane fixture is qualified")
    require(isinstance(value, list) and len(value) in lengths
        and all(number(size) and size > 0 for size in value), "malformed pane sizes")
    require(value[0] >= 200 and value[1] >= 400, "pane minimum constraints")
    return value


def validate_sample(original, actual, sample):
    before, after = sizes(original, (2, 3)), sizes(actual, (3,))
    require(sample.get("config", {}).get("ui", {}).get("paneSizes") == after, "stale config receipt")
    require(sample["config"]["ui"].get("assistantOpen") is False, "assistant visibility changed")
    require(after[2] == (before[2] if len(before) == 3 else ASSISTANT_DEFAULT), "hidden assistant size changed")
    for name in ("body", "activity", "split"):
        rect = sample.get(name)
        require(isinstance(rect, dict) and all(number(rect.get(key)) for key in ("x", "y", "width", "height")), "missing measured rectangle")
        require(rect["width"] > 0 and rect["height"] > 0, "invisible layout")
    viewport, outer, dpr = sample.get("viewport", {}), sample.get("outer", {}), sample.get("dpr")
    require(isinstance(viewport, dict) and isinstance(outer, dict), "missing viewport/window")
    require(number(dpr) and dpr > 0, "invalid backing scale")
    for key, expected in (("width", 1200), ("height", 820)):
        close(outer.get(key), expected)
        require(number(viewport.get(key)) and viewport[key] > 0, "invalid viewport")
        require(abs(viewport[key] * dpr - expected) <= 2, "viewport/backing scale/window mismatch")
    body, activity, split = (sample[key] for key in ("body", "activity", "split"))
    close(body["x"], 0)
    close(body["width"], viewport["width"])
    require(body["y"] >= 0 and body["y"] + body["height"] <= viewport["height"] + 1, "body outside viewport")
    # The authored 48px activity rail is a shrinkable flex sibling of a
    # width:100% split. Accept its measured CSS flex range, not a guessed width.
    minimum_rail = 48 * viewport["width"] / (viewport["width"] + 48)
    require(minimum_rail - 1 <= activity["width"] <= 49, "activity rail geometry")
    close(activity["x"], body["x"])
    close(split["x"], activity["x"] + activity["width"])
    close(split["width"] + activity["width"], body["width"])
    for rect in (activity, split):
        close(rect["y"], body["y"])
        close(rect["height"], body["height"])
    panes = sample.get("panes")
    require(isinstance(panes, list) and len(panes) == 2, "expected exactly two visible panes")
    offset = split["x"]
    for index, pane in enumerate(panes):
        require(isinstance(pane, dict), "missing pane rectangle")
        close(pane.get("x"), offset)
        close(pane.get("y"), split["y"])
        close(pane.get("height"), split["height"])
        close(pane.get("width"), after[index])
        # Allotment's default proportional reflow, with pixel rounding. The
        # fixture stays above both minimums, so no clamping is being excused.
        close(pane["width"], split["width"] * before[index] / sum(before[:2]))
        offset += pane["width"]
    close(offset, split["x"] + split["width"])


def validate(original, actual, observation, phase):
    require(phase in PHASES and isinstance(observation, dict) and observation.get("phase") == phase, "wrong phase")
    samples = observation.get("samples")
    require(isinstance(samples, list) and len(samples) == 2 and all(isinstance(sample, dict) for sample in samples), "two measured samples required")
    times = [sample.get("observedAtMs") for sample in samples]
    require(all(number(value) for value in times) and times[1] - times[0] >= 350, "debounce not observed")
    for sample in samples:
        validate_sample(original, actual, sample)
    for key in ("viewport", "outer", "dpr", "body", "activity", "split", "panes"):
        require(samples[0][key] == samples[1][key], "layout did not settle")


def startup_normalization(original, actual, observation, phase):
    require(phase in STARTUP_PHASES, "startup exception is forbidden after native work")
    validate(original, actual, observation, phase)
    before, after = original["ui"]["paneSizes"], actual["ui"]["paneSizes"]
    return [] if before == after else [dict(path="ui.paneSizes", before=before, after=after,
        phase=phase, cause="Measured Allotment proportional reflow and hidden assistant default")]


def require_restored(before, after):
    require(before.get("phase") == "electron-restart" and after.get("phase") == "electron-installed-rollback", "restoration phase mismatch")
    previous, current = before["samples"][-1], after["samples"][-1]
    for name in ("viewport", "outer", "body", "activity", "split"):
        for key, value in previous[name].items():
            close(value, current[name].get(key))
    require(previous["dpr"] == current["dpr"], "restored backing scale changed")
    for left, right in zip(previous["panes"], current["panes"], strict=True):
        for key, value in left.items():
            close(value, right.get(key))


def collect(cdp, process, wait_for, original, directory, phase, write):
    def settled():
        samples = []
        for index in range(2):
            if index:
                time.sleep(.4)  # Past the real hook's 300ms persistence debounce.
            sample = cdp.evaluate(SCRIPT)
            sample["observedAtMs"] = time.monotonic() * 1000
            samples.append(sample)
        observation = dict(phase=phase, samples=samples)
        write(directory / "layout-latest.json", observation)
        try:
            validate(original, samples[-1]["config"], observation, phase)
        except RuntimeError:
            return False
        return observation
    result = wait_for(settled, process, timeout=10)
    write(directory / "layout-observation.json", result)
    return result
