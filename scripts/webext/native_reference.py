"""Match only pinned native evaluation errors reproduced by Chromium."""
import json
import pathlib
import tomllib


def matching_evaluations(rows, reference):
    expected = reference.get('repeatedEvaluation')
    if not expected:
        return set()
    # proton_pass.json cites instrumented trace lines 443/455/456 and passive 162.
    source = 'chrome-extension://' + reference['package']['id'] + '/' + expected['source']
    seen = {}
    matched = set()
    for index, row in enumerate(rows):
        event = row.get('event', {})
        scope = event.get('scope', {})
        if (event.get('kind') != 'native-evaluation' or event.get('source') != source or
                row.get('extension') != reference['package']['id'] or
                row.get('context') != 'ContentScript'):
            continue
        key = (row.get('process'), scope.get('page'), scope.get('frame'), scope.get('document'), scope.get('world'))
        previous = seen.setdefault(key, [])
        if (row.get('extension') == reference['package']['id'] and
                row.get('context') == 'ContentScript' and
                all(part is not None for part in key) and scope.get('world') and
                scope.get('trigger') == 'executeScript' and
                event.get('success') is False and
                event.get('message') == expected['message'] and
                source + ':' + str(expected['line']) + ':' in event.get('stack', '') and
                len(previous) + 1 == expected['evaluation'] and
                previous and previous[0].get('success') is expected['previousSuccess'] and
                previous[0].get('scope', {}).get('trigger') == 'executeScript' and
                type(scope.get('requestId')) is int and scope['requestId'] > 0 and
                type(previous[0].get('scope', {}).get('requestId')) is int and
                previous[0]['scope']['requestId'] > 0 and
                scope['requestId'] != previous[0].get('scope', {}).get('requestId') and
                len(matched) < expected['maximum']):
            matched.add(index)
        previous.append(event)
    return matched


def reference_for(extension):
    root = pathlib.Path(__file__).resolve().parents[2]
    pins = tomllib.loads((root / 'scripts/webext/pinned-crx.toml').read_text())['extension']
    for path in (root / 'eval/webext-conformance/reference/password-managers').glob('*.json'):
        reference = json.loads(path.read_text())
        if reference['package']['id'] == extension:
            pin = next(item for item in pins if item['id'] == extension)
            if any(pin[key] != reference['package'][key] for key in ('id', 'version', 'sha256')):
                raise ValueError('Chromium reference pin is stale')
            return reference
    return {}
