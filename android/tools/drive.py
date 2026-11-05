#!/usr/bin/env python3
"""Taps through the app on the Tern emulator by what the screen says. Test tool, not shipped."""
import os, re, subprocess, sys, time
import xml.etree.ElementTree as ET

# The profile build: release's code, installable beside the real app.
PKG = 'xyz.hollingworth.Tern.profile'
ADB = [os.path.expanduser('~/Android/Sdk/platform-tools/adb'), '-s', 'emulator-5580']
HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'build', 'shots')
os.makedirs(HERE, exist_ok=True)

def adb(*a, check=True):
    return subprocess.run(ADB + list(a), capture_output=True, text=True, check=check).stdout

def nodes():
    for _ in range(4):
        adb('shell', 'uiautomator', 'dump', '/sdcard/ui.xml', check=False)
        raw = adb('exec-out', 'cat', '/sdcard/ui.xml', check=False)
        if raw.startswith('<?xml'):
            return list(ET.fromstring(raw).iter('node'))
        time.sleep(0.4)
    raise SystemExit('no UI dump')

def bounds(n):
    a, b, c, d = map(int, re.findall(r'\d+', n.get('bounds')))
    return (a + c) // 2, (b + d) // 2

def find(label, exact=True):
    for n in nodes():
        for key in ('text', 'content-desc'):
            v = n.get(key) or ''
            if v == label or (not exact and label in v):
                return n
    return None

def tap(label, exact=True, long=False):
    for _ in range(12):
        n = find(label, exact)
        if n is not None:
            x, y = bounds(n)
            if long:
                adb('shell', 'input', 'swipe', str(x), str(y), str(x), str(y), '700')
            else:
                adb('shell', 'input', 'tap', str(x), str(y))
            time.sleep(0.5)
            return
        time.sleep(0.5)
    print('TEXTS:', [n.get('text') or n.get('content-desc') for n in nodes() if n.get('text') or n.get('content-desc')])
    raise SystemExit(f'not on screen: {label}')

def wait(label, exact=True):
    for _ in range(24):
        if find(label, exact) is not None:
            return
        time.sleep(0.5)
    print('TEXTS:', [n.get('text') or n.get('content-desc') for n in nodes() if n.get('text') or n.get('content-desc')])
    raise SystemExit(f'never showed: {label}')

def gone(label):
    for _ in range(24):
        if find(label) is None:
            return
        time.sleep(0.5)
    raise SystemExit(f'still there: {label}')

def type_in(text):
    adb('shell', 'input', 'text', text.replace(' ', '%s'))
    time.sleep(0.4)

def back():
    adb('shell', 'input', 'keyevent', 'KEYCODE_BACK')
    time.sleep(0.7)

def shot(name):
    time.sleep(0.6)
    adb('shell', 'screencap', '-p', '/sdcard/s.png')
    adb('pull', '/sdcard/s.png', os.path.join(HERE, name + '.png'))

def swipe(x1, y1, x2, y2, ms=400):
    adb('shell', 'input', 'swipe', str(x1), str(y1), str(x2), str(y2), str(ms))

def texts():
    return [n.get('text') or n.get('content-desc') for n in nodes() if n.get('text') or n.get('content-desc')]

def relaunch(clear=False):
    adb('shell', 'am', 'force-stop', PKG)
    if clear:
        adb('shell', 'pm', 'clear', PKG)
    adb('shell', 'am', 'start', '-n', PKG + '/xyz.hollingworth.tern.MainActivity')
    time.sleep(1.5)

def crashes():
    log = adb('logcat', '-d', check=False)
    return [l for l in log.splitlines() if 'FATAL' in l or ('AndroidRuntime' in l and ' E ' in l)]

def tour():
    relaunch(clear=True)
    tap('Email address')
    type_in('demo')
    tap('Sign in')
    wait('Photos from Saturday')
    shot('01-inbox')
    tap('Open folders'); wait('Check for new mail'); shot('02-folders')
    back()
    tap('Photos from Saturday'); wait('Reply'); shot('03-conversation')
    tap('More'); wait('Labels'); shot('04-more'); back()
    tap('Reply'); wait('Send'); shot('05-reply')
    tap('Close and keep as draft'); wait('Reply')
    back(); wait('Search')
    tap('Accounts', long=True); wait('1 selected'); shot('06-selected')
    tap('Labels'); wait('New label'); shot('07-labels'); tap('Travel'); time.sleep(0.5); shot('08-labelled'); back(); time.sleep(0.5)
    tap('Pin'); time.sleep(0.8); shot('09-pinned')
    tap('Search'); wait('Search mail'); type_in('invoice'); time.sleep(0.8); shot('10-search'); back(); back(); time.sleep(0.5)
    tap('Open folders'); tap('Settings'); wait('Default mail app'); shot('11-settings')
    swipe(540, 1800, 540, 500); time.sleep(0.4); shot('12-settings2')
    tap('Mail rules'); tap('Add rule'); wait('Which mail'); tap('+ Subject'); type_in('invoice'); time.sleep(0.8); shot('13-rule')
    print('crashes:', crashes())

def tour2():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday')
    time.sleep(1.5)
    adb('shell', 'screenrecord', '--time-limit', '1', '/sdcard/none.mp4', check=False)
    # Pull to refresh, held mid gesture for a look, then released.
    adb('shell', 'input', 'swipe', '540', '500', '540', '1000', '1500')
    time.sleep(0.2); shot('20-after-pull')
    # Offline, with the status line, then a pull that fails with Retry.
    adb('shell', 'svc', 'wifi', 'disable'); adb('shell', 'svc', 'data', 'disable'); time.sleep(3); shot('21-offline')
    adb('shell', 'svc', 'wifi', 'enable'); adb('shell', 'svc', 'data', 'enable'); time.sleep(5); shot('22-back-online')
    print('after network back:', [t for t in texts() if 'nline' in t or 'unread' in t])
    # A mail link from another app.
    adb('shell', 'am', 'start', '-a', 'android.intent.action.VIEW', '-d', 'mailto:sam@tuta.com?subject=From%20a%20link\\&body=Hi\\&attach=/etc/passwd')
    time.sleep(1.5)
    if find('Use a different app') is not None or find('Just once') is not None:
        shot('23a-chooser')
        tap('Tern'); time.sleep(0.5)
        if find('Just once') is not None: tap('Just once')
    wait('From a link'); shot('23-mailto')
    tap('Close and keep as draft'); time.sleep(1)
    # Rule writing.
    tap('Open folders'); tap('Settings'); wait('Default mail app')
    swipe(540, 1800, 540, 500); time.sleep(0.4)
    tap('Mail rules'); tap('Add rule'); wait('Which mail'); tap('+ Subject'); time.sleep(0.5); type_in('invoice'); time.sleep(1); shot('24-rule')
    tap('Save'); time.sleep(0.8); shot('25-rules')
    back(); back(); time.sleep(0.5)
    print('crashes:', crashes())

def role():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    tap('Open folders'); tap('Settings'); wait('Default mail app'); shot('26-before')
    tap('Use Tern'); time.sleep(1.5); shot('26-picker')
    print('picker:', texts()[:8])
    if find('Tern') is not None: tap('Tern'); time.sleep(0.7)
    tap('Always'); time.sleep(1.5)
    print('after Always:', texts()[:8])
    tap('Close and keep as draft'); time.sleep(1)
    wait('Default mail app'); print('settings says:', [t for t in texts() if 'mail' in t.lower() or 'Tern' in t][:5]); shot('26-after')
    print('crashes:', crashes())

def unfolded():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    adb('shell', 'wm', 'size', '1768x2208'); adb('shell', 'wm', 'density', '400'); time.sleep(2)
    shot('27-unfolded')
    tap('Photos from Saturday'); wait('Reply'); shot('28-unfolded-open')
    tap('Sam Okafor'); time.sleep(1); shot('29-unfolded-next')
    adb('shell', 'wm', 'size', 'reset'); adb('shell', 'wm', 'density', 'reset'); time.sleep(1)
    print('crashes:', crashes())

def frames():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    x, y = bounds(find('Photos from Saturday'))
    p = subprocess.Popen(ADB + ['shell', 'screenrecord', '--time-limit', '4', '--size', '360x800', '--bit-rate', '6000000', '/sdcard/t.mp4'])
    time.sleep(1.0)
    adb('shell', 'input', 'tap', str(x), str(y)); time.sleep(1.2)
    back(); time.sleep(0.6)
    p.wait(); time.sleep(0.5)
    adb('pull', '/sdcard/t.mp4', os.path.join(HERE, 't.mp4'))

def pull():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    adb('shell', 'input', 'motionevent', 'DOWN', '540', '500')
    for y in range(540, 980, 60):
        adb('shell', 'input', 'motionevent', 'MOVE', '540', str(y))
    time.sleep(0.3); shot('41-pulling')
    adb('shell', 'input', 'motionevent', 'UP', '540', '980')
    time.sleep(0.1); shot('42-released')

def job():
    """Runs the system's job now, as if its time had come."""
    out = adb('shell', 'dumpsys', 'jobscheduler')
    found = re.search(r'JOB androidx\.work\.systemjobscheduler:u0a\d+/(\d+):[^\n]*' + re.escape(PKG), out)
    if not found:
        raise SystemExit('no job scheduled')
    print('job', found.group(1), adb('shell', 'cmd', 'jobscheduler', 'run', '-f', '-n', 'androidx.work.systemjobscheduler', PKG, found.group(1)).strip())

def notify():
    relaunch(clear=True)
    adb('shell', 'pm', 'revoke', PKG, 'android.permission.POST_NOTIFICATIONS', check=False)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    print('before asking, jobs:', PKG in adb('shell', 'dumpsys', 'jobscheduler').split('Pending queue')[0] and 'androidx.work' in adb('shell', 'dumpsys', 'jobscheduler'))
    tap('Open folders'); tap('Settings'); wait('Default mail app'); shot('50-settings')
    n = find('Notifications')
    x, y = bounds(n)
    adb('shell', 'input', 'tap', '960', str(y)); time.sleep(2); shot('51-permission'); print(texts()[:6])
    tap('Allow'); time.sleep(1.5); shot('52-on')
    print('settings:', [t for t in texts() if 'Every' in t or 'new mail' in t.lower()][:4])
    back(); time.sleep(0.5)
    adb('shell', 'input', 'keyevent', 'KEYCODE_HOME'); time.sleep(2)
    job(); time.sleep(5)
    adb('shell', 'cmd', 'statusbar', 'expand-notifications'); time.sleep(1.5); shot('53-shade')
    print('shade:', [t for t in texts() if 'Sam' in t or 'Table' in t][:4])
    tap('Table booked for 12:30', exact=False); time.sleep(2.5); shot('54-opened')
    print('opened:', texts()[:6])
    print('notification gone:', 'Table booked' not in adb('shell', 'dumpsys', 'notification', '--noredact').split('Notification List')[-1][:4000])
    print('crashes:', crashes())

def fake_login():
    relaunch(clear=True)
    tap('Email address'); type_in('tern-android-test-nonexistent@tuta.com'); tap('Password'); type_in('wrong-password'); tap('Sign in')
    time.sleep(6); shot('30-login'); print([t for t in texts() if t][:12])

def perf():
    relaunch(clear=True)
    tap('Email address'); type_in('demo'); tap('Sign in'); wait('Photos from Saturday'); time.sleep(1)
    adb('shell', 'dumpsys', 'gfxinfo', PKG, 'reset')
    for _ in range(4):
        tap('Photos from Saturday'); wait('Reply'); time.sleep(0.5); back(); wait('Search'); time.sleep(0.5)
    print(adb('shell', 'dumpsys', 'gfxinfo', PKG))
    times = []
    for _ in range(3):
        adb('shell', 'am', 'force-stop', PKG); time.sleep(1)
        out = adb('shell', 'am', 'start', '-W', '-n', PKG + '/xyz.hollingworth.tern.MainActivity')
        times.append(re.search(r'TotalTime: (\d+)', out).group(1)); time.sleep(2)
    print('cold starts ms:', times)

if __name__ == '__main__':
    globals()[sys.argv[1]](*sys.argv[2:])
