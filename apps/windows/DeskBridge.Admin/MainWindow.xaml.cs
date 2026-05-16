using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Windows;
using System.Windows.Input;
using System.Windows.Media;

namespace DeskBridge.Admin;

public partial class MainWindow : Window
{
    private Point? _dragStartPoint;
    private double _dragStartOffsetX;
    private double _dragStartOffsetY;
    private double _dragStartScale = MainWindowModel.MinimumPreviewScale;

    public MainWindow()
    {
        InitializeComponent();
        var model = new MainWindowModel();
        DataContext = model;
        Closed += (_, _) => model.Shutdown();
    }

    private MainWindowModel Model => (MainWindowModel)DataContext;

    private void PeerScreen_MouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        BeginScreenDrag(sender, e);
    }

    private void PeerScreen_MouseMove(object sender, MouseEventArgs e)
    {
        ContinueScreenDrag(e);
    }

    private void PeerScreen_MouseLeftButtonUp(object sender, MouseButtonEventArgs e)
    {
        EndScreenDrag(e);
    }

    private void BeginScreenDrag(object sender, MouseButtonEventArgs e)
    {
        _dragStartPoint = e.GetPosition(LayoutCanvas);
        _dragStartOffsetX = Model.PeerOffsetX;
        _dragStartOffsetY = Model.PeerOffsetY;
        _dragStartScale = Model.PreviewScale;
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
        var deltaX = (current.X - start.X) / _dragStartScale;
        var deltaY = (current.Y - start.Y) / _dragStartScale;

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
    private bool _clipboardEnabled = true;
    private bool _clipboardText = true;
    private bool _clipboardImage = true;
    private bool _clipboardFiles = true;
    private string _serverName = "windows";
    private string _allowedClient = "mac";
    private string _clientServerAddress = "192.168.2.5:24800";
    private readonly double _localWidth;
    private readonly double _localHeight;
    private double _peerWidth = DefaultPeerWidth;
    private double _peerHeight = DefaultPeerHeight;
    private double _peerOffsetX;
    private double _peerOffsetY;

    public const double MinimumPreviewScale = 0.02;
    private const double MaximumPreviewScale = 0.10;
    private const double DefaultLocalWidth = 1920;
    private const double DefaultLocalHeight = 1080;
    private const double DefaultPeerWidth = 1728;
    private const double DefaultPeerHeight = 1117;
    private const double PreviewCanvasWidth = 760;
    private const double PreviewCanvasHeight = 190;
    private const double PreviewPadding = 18;
    private const double MinOverlap = 140;
    private const double PortalGlowThickness = 5;
    private const int SM_CXSCREEN = 0;
    private const int SM_CYSCREEN = 1;
    private const int SM_CXVIRTUALSCREEN = 78;
    private const int SM_CYVIRTUALSCREEN = 79;

    public event PropertyChangedEventHandler? PropertyChanged;

    public MainWindowModel()
    {
        (_localWidth, _localHeight) = ReadPlatformScreenSize();
        _peerOffsetX = _localWidth;
        LoadConfig();
    }

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
        set
        {
            if (SetField(ref _reverseScroll, value))
            {
                ApplyRuntimeInputSettings();
            }
        }
    }

    public bool ClipboardEnabled
    {
        get => _clipboardEnabled;
        set => SetField(ref _clipboardEnabled, value);
    }

    public bool ClipboardText
    {
        get => _clipboardText;
        set => SetField(ref _clipboardText, value);
    }

    public bool ClipboardImage
    {
        get => _clipboardImage;
        set => SetField(ref _clipboardImage, value);
    }

    public bool ClipboardFiles
    {
        get => _clipboardFiles;
        set => SetField(ref _clipboardFiles, value);
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

    public string LayoutSummary => $"Server layout: {RouteSummary}; {PortalSummary()}";

    public double PreviewScale => ComputePreviewScale();
    public double LocalBoxLeft => PreviewOriginX(-PreviewBoundsLeft());
    public double LocalBoxTop => PreviewOriginY(-PreviewBoundsTop());
    public double LocalBoxWidth => LocalWidth * PreviewScale;
    public double LocalBoxHeight => LocalHeight * PreviewScale;
    public double PeerBoxLeft => LocalBoxLeft + PeerOffsetX * PreviewScale;
    public double PeerBoxTop => LocalBoxTop + PeerOffsetY * PreviewScale;
    public double PeerBoxWidth => PeerWidth * PreviewScale;
    public double PeerBoxHeight => PeerHeight * PreviewScale;

    public double LocalGlowLeft => GlowLeft(true);
    public double LocalGlowTop => GlowTop(true);
    public double LocalGlowWidth => GlowWidth(true);
    public double LocalGlowHeight => GlowHeight(true);
    public double PeerGlowLeft => GlowLeft(false);
    public double PeerGlowTop => GlowTop(false);
    public double PeerGlowWidth => GlowWidth(false);
    public double PeerGlowHeight => GlowHeight(false);

    private double LocalWidth => _localWidth;
    private double LocalHeight => _localHeight;
    private double PeerWidth => _peerWidth;
    private double PeerHeight => _peerHeight;

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
            ? new { width = (int)Math.Round(LocalWidth), height = (int)Math.Round(LocalHeight) }
            : new { width = (int)Math.Round(PeerWidth), height = (int)Math.Round(PeerHeight) };
        var clientSize = IsServerMode
            ? new { width = (int)Math.Round(PeerWidth), height = (int)Math.Round(PeerHeight) }
            : new { width = (int)Math.Round(LocalWidth), height = (int)Math.Round(LocalHeight) };

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
            clipboard = new
            {
                enabled = ClipboardEnabled,
                text = ClipboardText,
                image = ClipboardImage,
                files = ClipboardFiles,
                poll_ms = 750,
                max_transfer_bytes = 33554432,
            },
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

    private void LoadConfig()
    {
        if (!File.Exists(ConfigPath))
        {
            return;
        }

        try
        {
            var root = JsonNode.Parse(File.ReadAllText(ConfigPath))?.AsObject();
            if (root is null)
            {
                return;
            }

            var server = root["server"] as JsonObject;
            var client = root["client"] as JsonObject;
            var input = root["input"] as JsonObject;
            var clipboard = root["clipboard"] as JsonObject;
            var layout = root["layout"] as JsonObject;
            var screens = layout?["screens"] as JsonArray;

            var serverName = ReadString(server, "name");
            var clientName = ReadString(client, "name");

            if (!string.IsNullOrWhiteSpace(serverName))
            {
                _serverName = serverName;
            }
            if (!string.IsNullOrWhiteSpace(clientName))
            {
                _allowedClient = clientName;
            }
            if (ReadString(server, "listen") is { Length: > 0 } listen)
            {
                ListenAddress = listen;
            }
            if (ReadString(client, "server_addr") is { Length: > 0 } serverAddress)
            {
                _clientServerAddress = serverAddress;
            }
            if (ReadBool(input, "reverse_scroll") is { } reverseScroll)
            {
                _reverseScroll = reverseScroll;
            }
            if (ReadBool(clipboard, "enabled") is { } clipboardEnabled)
            {
                _clipboardEnabled = clipboardEnabled;
            }
            if (ReadBool(clipboard, "text") is { } clipboardText)
            {
                _clipboardText = clipboardText;
            }
            if (ReadBool(clipboard, "image") is { } clipboardImage)
            {
                _clipboardImage = clipboardImage;
            }
            if (ReadBool(clipboard, "files") is { } clipboardFiles)
            {
                _clipboardFiles = clipboardFiles;
            }
            if (screens is not null)
            {
                var peerSize = IsServerMode
                    ? FindScreenSize(screens, AllowedClient)
                    : FindScreenSize(screens, ServerName);
                if (peerSize is { } size)
                {
                    SetPeerSizeFields(size.Width, size.Height);
                }
            }

            ApplySavedLayout(layout);
            Diagnostics = $"Loaded config:\n{ConfigPath}";
        }
        catch (Exception ex)
        {
            Diagnostics = $"Could not load config:\n{ConfigPath}\n\n{ex.Message}";
        }
    }

    private void ApplySavedLayout(JsonObject? layout)
    {
        if (layout?["screens"] is not JsonArray screens)
        {
            return;
        }

        var serverOrigin = FindScreenOrigin(screens, ServerName);
        var clientOrigin = FindScreenOrigin(screens, AllowedClient);
        if (serverOrigin is { } server && clientOrigin is { } client)
        {
            SetPeerOffsetFields(client.X - server.X, client.Y - server.Y);
            return;
        }

        if (layout?["links"] is JsonArray links)
        {
            foreach (var node in links.OfType<JsonObject>())
            {
                if (ReadString(node, "from") != ServerName || ReadString(node, "to") != AllowedClient)
                {
                    continue;
                }

                switch (ReadString(node, "edge"))
                {
                    case "left":
                        SetPeerOffsetFields(-PeerWidth, 0);
                        return;
                    case "right":
                        SetPeerOffsetFields(LocalWidth, 0);
                        return;
                    case "top":
                        SetPeerOffsetFields(0, -PeerHeight);
                        return;
                    case "bottom":
                        SetPeerOffsetFields(0, LocalHeight);
                        return;
                }
            }
        }
    }

    private void ApplyRuntimeInputSettings()
    {
        SaveConfig();

        if (!IsServerMode)
        {
            return;
        }

        if (_serverProcess is not { HasExited: false })
        {
            return;
        }

        var daemon = LocateDaemon();
        if (!File.Exists(daemon))
        {
            return;
        }

        var localServer = $"127.0.0.1:{ListenPort()}";
        var targetName = AllowedClient;
        var reverseScroll = ReverseScroll.ToString().ToLowerInvariant();

        _ = Task.Run(() => RunDaemonCommand(
            daemon,
            new[]
            {
                "debug",
                "--server",
                localServer,
                "--name",
                targetName,
                "input-settings",
                "--reverse-scroll",
                reverseScroll,
            },
            3000)).ContinueWith(task =>
        {
            Application.Current.Dispatcher.Invoke(() =>
            {
                Diagnostics = $"Applied runtime input settings:\n{task.Result}";
            });
        });
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
            $"Clipboard: enabled={ClipboardEnabled} text={ClipboardText} image={ClipboardImage} files={ClipboardFiles}\n" +
            $"Admin display model: local={Math.Round(LocalWidth)}x{Math.Round(LocalHeight)} peer={Math.Round(PeerWidth)}x{Math.Round(PeerHeight)} offset=({Math.Round(PeerOffsetX)},{Math.Round(PeerOffsetY)}) {PortalSummary()}\n" +
            $"Daemon: {daemon}\nDeskBridge processes:\n{DescribeDeskBridgeProcesses()}",
        };

        if (File.Exists(daemon))
        {
            sections.Add("Local daemon version:\n" + RunDaemonCommand(daemon, new[] { "version" }, 3000));
            sections.Add("Runtime input settings:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "input-settings" }));
            sections.Add("Server debug log:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "server-logs" }));
            sections.Add("Route status:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "route-status" }));
            sections.Add("Performance:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "perf" }));
            sections.Add("Client peer info:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "peer-info" }));
            sections.Add("Client recent log:\n" + RunDaemonCommand(daemon, new[] { "debug", "--server", localServer, "--name", targetName, "logs" }));
        }

        sections.Add(
            "From the Mac, run:\n" +
            $"deskbridge diag --server <WINDOWS_LAN_IP>:{port} --name mac\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac input-settings\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac server-logs\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac route-status\n" +
            $"deskbridge debug --server <WINDOWS_LAN_IP>:{port} --name mac perf\n" +
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
        SetPeerOffsetFields(x, y);
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
        OnPropertyChanged(nameof(PeerOffsetX));
        OnPropertyChanged(nameof(PeerOffsetY));
        OnPropertyChanged(nameof(PreviewScale));
        OnPropertyChanged(nameof(LocalBoxLeft));
        OnPropertyChanged(nameof(LocalBoxTop));
        OnPropertyChanged(nameof(PeerBoxLeft));
        OnPropertyChanged(nameof(PeerBoxTop));
        OnPropertyChanged(nameof(LocalBoxWidth));
        OnPropertyChanged(nameof(LocalBoxHeight));
        OnPropertyChanged(nameof(PeerBoxWidth));
        OnPropertyChanged(nameof(PeerBoxHeight));
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

    private void SetPeerOffsetFields(double x, double y)
    {
        _peerOffsetX = Math.Clamp(x, -PeerWidth, LocalWidth);
        _peerOffsetY = Math.Clamp(y, -PeerHeight, LocalHeight);
    }

    private void SetPeerSizeFields(double width, double height)
    {
        _peerWidth = Math.Max(1, width);
        _peerHeight = Math.Max(1, height);
        SetPeerOffsetFields(PeerOffsetX, PeerOffsetY);
    }

    private double ComputePreviewScale()
    {
        var boundsWidth = Math.Max(1, PreviewBoundsRight() - PreviewBoundsLeft());
        var boundsHeight = Math.Max(1, PreviewBoundsBottom() - PreviewBoundsTop());
        var widthScale = (PreviewCanvasWidth - PreviewPadding * 2) / boundsWidth;
        var heightScale = (PreviewCanvasHeight - PreviewPadding * 2) / boundsHeight;
        return Math.Max(MinimumPreviewScale, Math.Min(MaximumPreviewScale, Math.Min(widthScale, heightScale)));
    }

    private double PreviewOriginX(double localOffset)
    {
        var scale = PreviewScale;
        var boundsWidth = PreviewBoundsRight() - PreviewBoundsLeft();
        var scaledWidth = boundsWidth * scale;
        return Math.Max(PreviewPadding, (PreviewCanvasWidth - scaledWidth) / 2) + localOffset * scale;
    }

    private double PreviewOriginY(double localOffset)
    {
        var scale = PreviewScale;
        var boundsHeight = PreviewBoundsBottom() - PreviewBoundsTop();
        var scaledHeight = boundsHeight * scale;
        return Math.Max(PreviewPadding, (PreviewCanvasHeight - scaledHeight) / 2) + localOffset * scale;
    }

    private double PreviewBoundsLeft() => Math.Min(0, PeerOffsetX);
    private double PreviewBoundsTop() => Math.Min(0, PeerOffsetY);
    private double PreviewBoundsRight() => Math.Max(LocalWidth, PeerOffsetX + PeerWidth);
    private double PreviewBoundsBottom() => Math.Max(LocalHeight, PeerOffsetY + PeerHeight);

    private string PortalSummary()
    {
        var edge = LocalToPeerEdge();
        if (edge is "left" or "right")
        {
            var start = OverlapTop();
            var end = OverlapBottom();
            if (end <= start)
            {
                return "portal: no vertical overlap";
            }

            return $"portal: y {Math.Round(start)}-{Math.Round(end)} of {Math.Round(LocalHeight)}";
        }

        var xStart = OverlapLeft();
        var xEnd = OverlapRight();
        if (xEnd <= xStart)
        {
            return "portal: no horizontal overlap";
        }

        return $"portal: x {Math.Round(xStart)}-{Math.Round(xEnd)} of {Math.Round(LocalWidth)}";
    }

    private double OverlapLeft() => Math.Max(0, PeerOffsetX);
    private double OverlapRight() => Math.Min(LocalWidth, PeerOffsetX + PeerWidth);
    private double OverlapTop() => Math.Max(0, PeerOffsetY);
    private double OverlapBottom() => Math.Min(LocalHeight, PeerOffsetY + PeerHeight);

    private static string? ReadString(JsonObject? obj, string key)
    {
        try
        {
            return obj?[key]?.GetValue<string>();
        }
        catch
        {
            return null;
        }
    }

    private static bool? ReadBool(JsonObject? obj, string key)
    {
        try
        {
            return obj?[key]?.GetValue<bool>();
        }
        catch
        {
            return null;
        }
    }

    private static (double X, double Y)? FindScreenOrigin(JsonArray screens, string screenName)
    {
        foreach (var screen in screens.OfType<JsonObject>())
        {
            if (ReadString(screen, "name") != screenName)
            {
                continue;
            }

            if (screen["origin"] is JsonObject origin
                && ReadNumber(origin, "x") is { } x
                && ReadNumber(origin, "y") is { } y)
            {
                return (x, y);
            }
        }

        return null;
    }

    private static (double Width, double Height)? FindScreenSize(JsonArray screens, string screenName)
    {
        foreach (var screen in screens.OfType<JsonObject>())
        {
            if (ReadString(screen, "name") != screenName)
            {
                continue;
            }

            if (screen["size"] is JsonObject size
                && ReadNumber(size, "width") is { } width
                && ReadNumber(size, "height") is { } height)
            {
                return (width, height);
            }
        }

        return null;
    }

    private static double? ReadNumber(JsonObject obj, string key)
    {
        var node = obj[key];
        if (node is null)
        {
            return null;
        }

        try
        {
            return JsonSerializer.Deserialize<double>(node.ToJsonString());
        }
        catch
        {
            return null;
        }
    }

    private static (double Width, double Height) ReadPlatformScreenSize()
    {
        try
        {
            _ = SetProcessDPIAware();
            var width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            var height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            if (width <= 0 || height <= 0)
            {
                width = GetSystemMetrics(SM_CXSCREEN);
                height = GetSystemMetrics(SM_CYSCREEN);
            }
            if (width > 0 && height > 0)
            {
                return (width, height);
            }
        }
        catch
        {
            // Fall through to conservative defaults when the Win32 query is unavailable.
        }

        return (DefaultLocalWidth, DefaultLocalHeight);
    }

    [DllImport("user32.dll")]
    private static extern int GetSystemMetrics(int nIndex);

    [DllImport("user32.dll")]
    private static extern bool SetProcessDPIAware();

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
        var scale = PreviewScale;
        return edge switch
        {
            "right" => left + width - PortalGlowThickness,
            "top" or "bottom" => left + PortalLeftForScreen(local) * scale,
            _ => left,
        };
    }

    private double GlowTop(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var top = local ? LocalBoxTop : PeerBoxTop;
        var height = local ? LocalBoxHeight : PeerBoxHeight;
        var scale = PreviewScale;
        return edge switch
        {
            "left" or "right" => top + PortalTopForScreen(local) * scale,
            "bottom" => top + height - PortalGlowThickness,
            _ => top,
        };
    }

    private double GlowWidth(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var scale = PreviewScale;
        return edge switch
        {
            "left" or "right" => PortalGlowThickness,
            "top" or "bottom" => Math.Max(0, (OverlapRight() - OverlapLeft()) * scale),
            _ => local ? LocalBoxWidth : PeerBoxWidth,
        };
    }

    private double GlowHeight(bool local)
    {
        var edge = local ? LocalToPeerEdge() : Opposite(LocalToPeerEdge());
        var height = local ? LocalBoxHeight : PeerBoxHeight;
        var scale = PreviewScale;
        return edge switch
        {
            "left" or "right" => Math.Max(0, (OverlapBottom() - OverlapTop()) * scale),
            "top" or "bottom" => PortalGlowThickness,
            _ => height,
        };
    }

    private double PortalLeftForScreen(bool local) => local ? OverlapLeft() : OverlapLeft() - PeerOffsetX;

    private double PortalTopForScreen(bool local) => local ? OverlapTop() : OverlapTop() - PeerOffsetY;

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
