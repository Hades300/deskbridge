using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.CompilerServices;
using System.Text.Json;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace DeskBridge.Admin;

public partial class MainWindow : Window
{
    private Point? _dragStartPoint;
    private double _dragStartOffsetX;
    private double _dragStartOffsetY;
    private bool _draggingLocalScreen;

    public MainWindow()
    {
        InitializeComponent();
        var model = new MainWindowModel();
        DataContext = model;
        Closed += (_, _) => model.Shutdown();
    }

    private MainWindowModel Model => (MainWindowModel)DataContext;

    private void LocalScreen_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        BeginScreenDrag(sender, e, draggingLocalScreen: true);
    }

    private void LocalScreen_MouseMove(object sender, MouseEventArgs e)
    {
        ContinueScreenDrag(e);
    }

    private void LocalScreen_MouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        EndScreenDrag(e);
    }

    private void PeerScreen_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        BeginScreenDrag(sender, e, draggingLocalScreen: false);
    }

    private void PeerScreen_MouseMove(object sender, MouseEventArgs e)
    {
        ContinueScreenDrag(e);
    }

    private void PeerScreen_MouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        EndScreenDrag(e);
    }

    private void BeginScreenDrag(object sender, MouseButtonEventArgs e, bool draggingLocalScreen)
    {
        _dragStartPoint = e.GetPosition(LayoutCanvas);
        _dragStartOffsetX = Model.PeerOffsetX;
        _dragStartOffsetY = Model.PeerOffsetY;
        _draggingLocalScreen = draggingLocalScreen;
        Mouse.Capture((IInputElement)sender);
        e.Handled = true;
    }

    private void ContinueScreenDrag(MouseEventArgs e)
    {
        if (_dragStartPoint is not { } start || e.LeftButton != MouseButtonState.Pressed)
        {
            return;
        }

        var current = e.GetPosition(LayoutCanvas);
        var deltaX = (current.X - start.X) / MainWindowModel.LayoutScale;
        var deltaY = (current.Y - start.Y) / MainWindowModel.LayoutScale;
        if (_draggingLocalScreen)
        {
            deltaX = -deltaX;
            deltaY = -deltaY;
        }

        Model.SetPeerOffset(
            _dragStartOffsetX + deltaX,
            _dragStartOffsetY + deltaY);
    }

    private void EndScreenDrag(MouseButtonEventArgs e)
    {
        _dragStartPoint = null;
        Model.SnapPeerToNearestEdge();
        Mouse.Capture(null);
        e.Handled = true;
    }
}

public sealed class MainWindowModel : INotifyPropertyChanged
{
    private Process? _serverProcess;
    private string _mode = "Server";
    private string _statusText = "Stopped";
    private Brush _statusBrush = Brushes.Orange;
    private string _diagnostics = "No diagnostics yet.";
    private bool _captureInput = true;
    private bool _debugLogging = true;
    private bool _reverseScroll;
    private string _serverName = "windows";
    private string _allowedClient = "mac";
    private string _clientServerAddress = "192.168.2.5:24800";
    private double _peerOffsetX = 1920;
    private double _peerOffsetY;

    public const double LayoutScale = 0.08;
    private const double LocalWidth = 1920;
    private const double LocalHeight = 1080;
    private const double PeerWidth = 1728;
    private const double PeerHeight = 1117;
    private const double LocalCanvasLeft = 250;
    private const double LocalCanvasTop = 56;
    private const double MinOverlap = 140;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string ListenAddress { get; set; } = "0.0.0.0:24800";

    public string ConfigPath { get; set; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "DeskBridge", "deskbridge.json");

    public string Mode
    {
        get => _mode;
        set
        {
            if (SetField(ref _mode, value))
            {
                OnModeChanged();
            }
        }
    }

    public bool IsServerMode => Mode == "Server";

    public Visibility ServerSettingsVisibility => IsServerMode ? Visibility.Visible : Visibility.Collapsed;

    public Visibility ClientSettingsVisibility => IsServerMode ? Visibility.Collapsed : Visibility.Visible;

    public Visibility LayoutVisibility => ServerSettingsVisibility;

    public string StartButtonText => IsServerMode ? "Start Server" : "Start Client";

    public string StopButtonText => IsServerMode ? "Stop Server" : "Stop Client";

    public string LocalNameLabel => IsServerMode ? "Server" : "Client";

    public string PeerNameLabel => IsServerMode ? "Allowed" : "Server";

    public string WindowTitle => IsServerMode ? "DeskBridge Server" : "DeskBridge Client";

    public string LocalDisplayName => IsServerMode ? ServerName : AllowedClient;

    public string PeerDisplayName => IsServerMode ? AllowedClient : ServerName;

    public string PeerLayoutSummary => IsServerMode ? "Saved on this server" : "Configured on the server";

    public bool CaptureInput
    {
        get => _captureInput;
        set => SetField(ref _captureInput, value);
    }

    public bool DebugLogging
    {
        get => _debugLogging;
        set => SetField(ref _debugLogging, value);
    }

    public bool ReverseScroll
    {
        get => _reverseScroll;
        set => SetField(ref _reverseScroll, value);
    }

    public string ServerName
    {
        get => _serverName;
        set
        {
            if (SetField(ref _serverName, value))
            {
                OnPropertyChanged(nameof(RouteSummary));
                OnPropertyChanged(nameof(LayoutSummary));
                OnPropertyChanged(nameof(LocalDisplayName));
                OnPropertyChanged(nameof(PeerDisplayName));
            }
        }
    }

    public string AllowedClient
    {
        get => _allowedClient;
        set
        {
            if (SetField(ref _allowedClient, value))
            {
                OnPropertyChanged(nameof(RouteSummary));
                OnPropertyChanged(nameof(LayoutSummary));
                OnPropertyChanged(nameof(LocalDisplayName));
                OnPropertyChanged(nameof(PeerDisplayName));
            }
        }
    }

    public string ClientServerAddress
    {
        get => _clientServerAddress;
        set => SetField(ref _clientServerAddress, value);
    }

    public double PeerOffsetX
    {
        get => _peerOffsetX;
        private set => SetField(ref _peerOffsetX, value);
    }

    public double PeerOffsetY
    {
        get => _peerOffsetY;
        private set => SetField(ref _peerOffsetY, value);
    }

    public string LayoutArrow => LocalToPeerEdge() switch
    {
        "left" => "<-",
        "top" => "^",
        "bottom" => "v",
        _ => "->",
    };

    public string RouteSummary => $"{ServerName} {ServerLinkEdge().ToUpperInvariant()} -> {AllowedClient}";

    public string LayoutSummary => $"Server layout: {RouteSummary}";

    public double LocalBoxLeft => LocalCanvasLeft;
    public double LocalBoxTop => LocalCanvasTop;
    public double LocalBoxWidth => LocalWidth * LayoutScale;
    public double LocalBoxHeight => LocalHeight * LayoutScale;
    public double PeerBoxLeft => LocalCanvasLeft + PeerOffsetX * LayoutScale;
    public double PeerBoxTop => LocalCanvasTop + PeerOffsetY * LayoutScale;
    public double PeerBoxWidth => PeerWidth * LayoutScale;
    public double PeerBoxHeight => PeerHeight * LayoutScale;

    public double LocalGlowLeft => GlowLeft(true);
    public double LocalGlowTop => GlowTop(true);
    public double LocalGlowWidth => GlowWidth(true);
    public double LocalGlowHeight => GlowHeight(true);
    public double PeerGlowLeft => GlowLeft(false);
    public double PeerGlowTop => GlowTop(false);
    public double PeerGlowWidth => GlowWidth(false);
    public double PeerGlowHeight => GlowHeight(false);

    public string StatusText
    {
        get => _statusText;
        private set => SetField(ref _statusText, value);
    }

    public Brush StatusBrush
    {
        get => _statusBrush;
        private set => SetField(ref _statusBrush, value);
    }

    public string Diagnostics
    {
        get => _diagnostics;
        private set => SetField(ref _diagnostics, value);
    }

    public ICommand StartCommand => new RelayCommand(Start);
    public ICommand StopCommand => new RelayCommand(Stop);
    public ICommand SaveConfigCommand => new RelayCommand(SaveConfig);
    public ICommand DiagnoseCommand => new RelayCommand(Diagnose);
    public ICommand FirewallCommand => new RelayCommand(OpenFirewall);

    public void Shutdown()
    {
        Stop(killAllDaemons: true);
    }

    private void Start()
    {
        Stop(killAllDaemons: true);
        SaveConfig();

        var daemonPath = LocateDaemon();
        if (!File.Exists(daemonPath))
        {
            StatusText = "Daemon missing";
            StatusBrush = Brushes.Red;
            Diagnostics =
                "Could not find deskbridge.exe next to the admin app.\n" +
                $"Expected: {daemonPath}\n\n" +
                "Use the Windows release zip as-is, or place deskbridge.exe in the same folder as DeskBridge.Admin.exe.";
            return;
        }

        var daemonArgs = IsServerMode ? $"server --config \"{ConfigPath}\"" : $"client --config \"{ConfigPath}\" --reconnect";
        if (IsServerMode && CaptureInput)
        {
            daemonArgs += " --capture";
        }
        if (IsServerMode && DebugLogging)
        {
            daemonArgs += " --debug-capture-log";
        }
        if (IsServerMode && ReverseScroll)
        {
            daemonArgs += " --reverse-scroll";
        }

        var process = new Process
        {
            StartInfo = new ProcessStartInfo
            {
                FileName = daemonPath,
                Arguments = daemonArgs,
                WorkingDirectory = AppContext.BaseDirectory,
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true,
            },
            EnableRaisingEvents = true,
        };
        process.OutputDataReceived += (_, args) => Append(args.Data);
        process.ErrorDataReceived += (_, args) => Append(args.Data);
        process.Exited += (_, _) =>
        {
            Application.Current.Dispatcher.Invoke(() =>
            {
                if (ReferenceEquals(_serverProcess, process))
                {
                    _serverProcess = null;
                }
                StatusText = "Stopped";
                StatusBrush = Brushes.Orange;
            });
        };

        try
        {
            process.Start();
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();
            _serverProcess = process;
            StatusText = IsServerMode ? $"Running on {ListenAddress}" : $"Connected to {ClientServerAddress}";
            StatusBrush = Brushes.Green;
            Diagnostics =
                $"Started DeskBridge {Mode.ToLowerInvariant()}.\nArgs: {daemonArgs}\nScreen: {(IsServerMode ? ServerName : AllowedClient)}\nPeer: {(IsServerMode ? AllowedClient : ServerName)}\nReverse scroll: {IsServerMode && ReverseScroll}\nDaemon: {daemonPath}";
        }
        catch (Exception ex)
        {
            StatusText = "Start failed";
            StatusBrush = Brushes.Red;
            Diagnostics = ex.ToString();
        }
    }

    private void Stop()
    {
        Stop(killAllDaemons: true);
    }

    private void Stop(bool killAllDaemons)
    {
        var cleanupNotes = new List<string>();

        if (_serverProcess is { HasExited: false })
        {
            try
            {
                var pid = _serverProcess.Id;
                _serverProcess.Kill(entireProcessTree: true);
                _serverProcess.WaitForExit(2000);
                cleanupNotes.Add($"Stopped tracked daemon pid {pid}.");
            }
            catch (Exception ex)
            {
                cleanupNotes.Add(ex.ToString());
            }
        }
        _serverProcess = null;

        if (killAllDaemons)
        {
            cleanupNotes.AddRange(KillDeskBridgeDaemons());
        }

        StatusText = "Stopped";
        StatusBrush = Brushes.Orange;
        if (cleanupNotes.Count > 0)
        {
            Diagnostics = string.Join("\n", cleanupNotes);
        }
    }

    private void SaveConfig()
    {
        var edge = ServerLinkEdge();
        var serverOrigin = IsServerMode
            ? new { x = 0, y = 0 }
            : new { x = (int)Math.Round(PeerOffsetX), y = (int)Math.Round(PeerOffsetY) };
        var clientOrigin = IsServerMode
            ? new { x = (int)Math.Round(PeerOffsetX), y = (int)Math.Round(PeerOffsetY) }
            : new { x = 0, y = 0 };
        var serverSize = IsServerMode
            ? new { width = (int)LocalWidth, height = (int)LocalHeight }
            : new { width = (int)PeerWidth, height = (int)PeerHeight };
        var clientSize = IsServerMode
            ? new { width = (int)PeerWidth, height = (int)PeerHeight }
            : new { width = (int)LocalWidth, height = (int)LocalHeight };

        var config = new
        {
            server = new { name = ServerName, listen = ListenAddress },
            client = new { name = AllowedClient, server_addr = ClientServerAddress },
            layout = new
            {
                screens = new object[]
                {
                    new { name = ServerName, size = serverSize, origin = serverOrigin },
                    new { name = AllowedClient, size = clientSize, origin = clientOrigin },
                },
                links = new object[] { new { from = ServerName, edge, to = AllowedClient } },
            },
            reliability = new { heartbeat_ms = 2000, reconnect_max_ms = 10000, stale_after_ms = 6000 },
            input = new { reverse_scroll = IsServerMode && ReverseScroll },
        };

        var directory = Path.GetDirectoryName(ConfigPath);
        if (!string.IsNullOrWhiteSpace(directory))
        {
            Directory.CreateDirectory(directory);
        }

        var json = JsonSerializer.Serialize(config, new JsonSerializerOptions { WriteIndented = true });
        File.WriteAllText(ConfigPath, json);
        Diagnostics = $"Wrote config:\n{ConfigPath}";
    }

    private void Diagnose()
    {
        var port = ListenPort();
        var daemon = LocateDaemon();
        var localServer = IsServerMode ? $"127.0.0.1:{port}" : ClientServerAddress;
        var targetName = IsServerMode ? AllowedClient : AllowedClient;
        var sections = new List<string>
        {
            $"Status: {StatusText}\nMode: {Mode}\nTracked daemon: {DescribeTrackedDaemon()}\nServer: {ListenAddress}\nClient server: {ClientServerAddress}\nRoute: {RouteSummary}\nDebug capture log: {DebugLogging}\nReverse scroll: {IsServerMode && ReverseScroll}\n" +
            $"Daemon: {daemon}\nDeskBridge processes:\n{DescribeDeskBridgeProcesses()}",
        };

        if (File.Exists(daemon))
        {
            sections.Add("Local daemon version:\n" + RunDaemonCommand(daemon, new[] { "version" }, 3000));
            sections.Add("Server debug log:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "server-logs" }));
            sections.Add("Route status:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "route-status" }));
            sections.Add("Client peer info:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "peer-info" }));
            sections.Add("Client recent log:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "logs" }));
        }

        sections.Add(
            "From the Mac, run:\n" +
            $"deskbridge diag --server <WINDOWS_LAN_IP>:{port} --name mac\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac server-logs\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac route-status\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac route-probe --steps 3 --dx 80 --dy -2\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac capture-probe --steps 3 --dx 80 --dy -2");

        sections.Add(
            "On Windows, verify the listener with:\n" +
            $"Get-NetTCPConnection -LocalPort {port} -State Listen\n\n" +
            "If logs show non-DeskBridge clients hitting this port, find the process with:\n" +
            $"Get-NetTCPConnection -RemotePort {port} -State Established | Select-Object LocalAddress,LocalPort,OwningProcess");

        Diagnostics = string.Join("\n\n---\n\n", sections);
    }

    private void OpenFirewall()
    {
        Diagnostics =
            "PowerShell firewall rule:\n" +
            $"New-NetFirewallRule -DisplayName \"DeskBridge TCP {ListenPort()}\" -Direction Inbound -Protocol TCP -LocalPort {ListenPort()} -Action Allow";
    }

    public void SetPeerOffset(double x, double y)
    {
        PeerOffsetX = x;
        PeerOffsetY = y;
        OnLayoutChanged();
    }

    public void SnapPeerToNearestEdge()
    {
        var localCenterX = LocalWidth / 2;
        var localCenterY = LocalHeight / 2;
        var peerCenterX = PeerOffsetX + PeerWidth / 2;
        var peerCenterY = PeerOffsetY + PeerHeight / 2;
        var dx = peerCenterX - localCenterX;
        var dy = peerCenterY - localCenterY;

        if (Math.Abs(dx / LocalWidth) >= Math.Abs(dy / LocalHeight))
        {
            PeerOffsetX = dx >= 0 ? LocalWidth : -PeerWidth;
            PeerOffsetY = Math.Clamp(PeerOffsetY, -PeerHeight + MinOverlap, LocalHeight - MinOverlap);
        }
        else
        {
            PeerOffsetY = dy >= 0 ? LocalHeight : -PeerHeight;
            PeerOffsetX = Math.Clamp(PeerOffsetX, -PeerWidth + MinOverlap, LocalWidth - MinOverlap);
        }

        OnLayoutChanged();
    }

    private void OnModeChanged()
    {
        OnPropertyChanged(nameof(IsServerMode));
        OnPropertyChanged(nameof(ServerSettingsVisibility));
        OnPropertyChanged(nameof(ClientSettingsVisibility));
        OnPropertyChanged(nameof(LayoutVisibility));
        OnPropertyChanged(nameof(StartButtonText));
        OnPropertyChanged(nameof(StopButtonText));
        OnPropertyChanged(nameof(LocalNameLabel));
        OnPropertyChanged(nameof(PeerNameLabel));
        OnPropertyChanged(nameof(WindowTitle));
        OnPropertyChanged(nameof(LocalDisplayName));
        OnPropertyChanged(nameof(PeerDisplayName));
        OnPropertyChanged(nameof(PeerLayoutSummary));
        OnPropertyChanged(nameof(RouteSummary));
        OnPropertyChanged(nameof(LayoutSummary));
    }

    private void OnLayoutChanged()
    {
        OnPropertyChanged(nameof(PeerBoxLeft));
        OnPropertyChanged(nameof(PeerBoxTop));
        OnPropertyChanged(nameof(LayoutArrow));
        OnPropertyChanged(nameof(RouteSummary));
        OnPropertyChanged(nameof(LayoutSummary));
        OnPropertyChanged(nameof(LocalGlowLeft));
        OnPropertyChanged(nameof(LocalGlowTop));
        OnPropertyChanged(nameof(LocalGlowWidth));
        OnPropertyChanged(nameof(LocalGlowHeight));
        OnPropertyChanged(nameof(PeerGlowLeft));
        OnPropertyChanged(nameof(PeerGlowTop));
        OnPropertyChanged(nameof(PeerGlowWidth));
        OnPropertyChanged(nameof(PeerGlowHeight));
    }

    private string LocalToPeerEdge()
    {
        return EdgeFrom(
            0,
            0,
            LocalWidth,
            LocalHeight,
            PeerOffsetX,
            PeerOffsetY,
            PeerWidth,
            PeerHeight);
    }

    private string ServerLinkEdge()
    {
        if (IsServerMode)
        {
            return LocalToPeerEdge();
        }

        return EdgeFrom(
            PeerOffsetX,
            PeerOffsetY,
            PeerWidth,
            PeerHeight,
            0,
            0,
            LocalWidth,
            LocalHeight);
    }

    private static string EdgeFrom(
        double fromX,
        double fromY,
        double fromWidth,
        double fromHeight,
        double toX,
        double toY,
        double toWidth,
        double toHeight)
    {
        var fromCenterX = fromX + fromWidth / 2;
        var fromCenterY = fromY + fromHeight / 2;
        var toCenterX = toX + toWidth / 2;
        var toCenterY = toY + toHeight / 2;
        var dx = toCenterX - fromCenterX;
        var dy = toCenterY - fromCenterY;

        if (Math.Abs(dx / Math.Max(fromWidth, 1)) >= Math.Abs(dy / Math.Max(fromHeight, 1)))
        {
            return dx >= 0 ? "right" : "left";
        }
        return dy >= 0 ? "bottom" : "top";
    }

    private double GlowLeft(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var left = local ? LocalBoxLeft : PeerBoxLeft;
        var width = local ? LocalBoxWidth : PeerBoxWidth;
        return edge switch
        {
            "right" => left + width - 5,
            _ => left,
        };
    }

    private double GlowTop(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var top = local ? LocalBoxTop : PeerBoxTop;
        var height = local ? LocalBoxHeight : PeerBoxHeight;
        return edge switch
        {
            "bottom" => top + height - 5,
            _ => top,
        };
    }

    private double GlowWidth(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var width = local ? LocalBoxWidth : PeerBoxWidth;
        return edge is "left" or "right" ? 5 : width;
    }

    private double GlowHeight(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var height = local ? LocalBoxHeight : PeerBoxHeight;
        return edge is "top" or "bottom" ? 5 : height;
    }

    private static string Opposite(string edge)
    {
        return edge switch
        {
            "left" => "right",
            "right" => "left",
            "top" => "bottom",
            "bottom" => "top",
            _ => "left",
        };
    }

    private void Append(string? line)
    {
        if (string.IsNullOrWhiteSpace(line)) return;
        Application.Current.Dispatcher.Invoke(() =>
        {
            var next = $"{Diagnostics}\n{line}";
            Diagnostics = next.Length > 20_000 ? next[^20_000..] : next;
        });
    }

    private static string LocateDaemon()
    {
        return Path.Combine(AppContext.BaseDirectory, "deskbridge.exe");
    }

    private static IReadOnlyList<string> KillDeskBridgeDaemons()
    {
        var notes = new List<string>();

        foreach (var process in Process.GetProcessesByName("deskbridge"))
        {
            string? processPath;
            try
            {
                processPath = process.MainModule?.FileName;
            }
            catch (Exception ex)
            {
                notes.Add($"Skipped deskbridge pid {process.Id}: cannot inspect path ({ex.Message}).");
                continue;
            }

            if (string.IsNullOrWhiteSpace(processPath))
            {
                continue;
            }

            try
            {
                if (!process.HasExited)
                {
                    var pid = process.Id;
                    process.Kill(entireProcessTree: true);
                    process.WaitForExit(2000);
                    notes.Add($"Stopped stale daemon pid {pid}: {processPath}");
                }
            }
            catch (Exception ex)
            {
                notes.Add($"Failed to stop deskbridge pid {process.Id}: {ex.Message}");
            }
        }

        return notes;
    }

    private string ListenPort()
    {
        var lastColon = ListenAddress.LastIndexOf(':');
        if (lastColon >= 0 && lastColon < ListenAddress.Length - 1)
        {
            return ListenAddress[(lastColon + 1)..];
        }

        return "24800";
    }

    private string DescribeTrackedDaemon()
    {
        return _serverProcess switch
        {
            null => "not tracked by this window",
            { HasExited: false } process => $"running, pid {process.Id}",
            { HasExited: true } process => $"exited, code {process.ExitCode}",
        };
    }

    private static string DescribeDeskBridgeProcesses()
    {
        var descriptions = Process.GetProcessesByName("deskbridge")
            .Select(process =>
            {
                try
                {
                    return $"pid {process.Id}: {process.MainModule?.FileName ?? process.ProcessName}";
                }
                catch
                {
                    return $"pid {process.Id}: {process.ProcessName}";
                }
            })
            .ToArray();

        return descriptions.Length == 0 ? "none found" : string.Join("\n", descriptions);
    }

    private static string RunDaemonCommand(string daemonPath, string[] arguments, int timeoutMs = 8000)
    {
        try
        {
            var process = new Process
            {
                StartInfo = new ProcessStartInfo
                {
                    FileName = daemonPath,
                    WorkingDirectory = AppContext.BaseDirectory,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                },
            };

            foreach (var argument in arguments)
            {
                process.StartInfo.ArgumentList.Add(argument);
            }

            process.Start();
            var outputTask = process.StandardOutput.ReadToEndAsync();
            var errorTask = process.StandardError.ReadToEndAsync();
            if (!process.WaitForExit(timeoutMs))
            {
                process.Kill(entireProcessTree: true);
                return $"Timed out after {timeoutMs}ms.";
            }

            var output = outputTask.GetAwaiter().GetResult();
            var error = errorTask.GetAwaiter().GetResult();
            var text = string.Join("\n", new[] { output, error }.Where(value => !string.IsNullOrWhiteSpace(value))).Trim();
            return string.IsNullOrWhiteSpace(text) ? $"Exited with code {process.ExitCode}." : text;
        }
        catch (Exception ex)
        {
            return ex.Message;
        }
    }

    private bool SetField<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(name);
        return true;
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null)
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    }
}

public sealed class RelayCommand(Action execute) : ICommand
{
    public event EventHandler? CanExecuteChanged
    {
        add { }
        remove { }
    }

    public bool CanExecute(object? parameter) => true;
    public void Execute(object? parameter) => execute();
}
