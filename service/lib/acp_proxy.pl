#!/usr/bin/perl
# The boundary between an ACP client on the host and the adapter in the box (box acp).
#
# usage: acp_proxy.pl <max line bytes> <end seconds> <stall seconds> <report file> <command...>
#
# The agent in the box is not trusted; the client that reads its messages may be careless. So
# the rules are applied here, between the two, line by line (one JSON-RPC message per line):
# - Client to agent: the `initialize` request loses every client capability that is not named
#   in PASSED_CAPABILITIES. `fs` and `terminal` are not, so the agent is never told that it may
#   read or write files or run commands on the client's side.
# - Agent to client: responses pass, notifications only of PASSED_NOTIFICATIONS, requests only
#   of PASSED_REQUESTS. Any other request (fs/*, terminal/*, anything unknown) is answered to
#   the agent with a JSON-RPC error and never reaches the client. Other notifications and lines
#   that are not a JSON-RPC message are dropped.
# - What passes is written again from what was parsed, so the client reads exactly the message
#   that was judged (no second reading of duplicate keys or odd encodings).
# - A line longer than the limit is never parsed or kept: the session is ended, exit code 65.
# - This process is the supervisor of the session. It keeps whatever lock it was started with
#   (a descriptor box leaves open for it) until the command has ended. When BOX_ACP_LIFETIME_FD
#   names a descriptor, it is the reading end of a pipe whose writing end only box holds: its
#   end of file means that box itself is gone (killed outright, no cleanup), and the session is
#   ended at once, as "front end gone".
# - Nothing is waited for without a limit. An agent that has not ended <end seconds> after the
#   client closed its side or a signal requested the end (whatever it still sends or reads), or a side that has not taken a byte of what waits for it in <stall seconds>,
#   ends the session, exit code 66. A session in which nobody speaks is not a stall.
# Refusals are logged on standard error and counted in the report file. Standard output carries
# nothing but the agent's messages that passed. The exit code is the command's otherwise.
use strict;
use warnings;
use Fcntl qw(F_GETFL F_SETFL O_NONBLOCK);
use IO::Select;
use JSON::PP;
use POSIX qw(EAGAIN EINTR);

use constant {
    READ_BYTES         => 65536,
    OVERSIZED_EXIT     => 65,
    STALLED_EXIT       => 66,
    TICK_SECONDS       => 1,
    METHOD_NOT_FOUND   => -32601,
    DRAIN_SECONDS      => 10,
    DRAIN_AFTER_EXIT_SECONDS => 2,
    MAX_INHERITED_FD   => 63,
    STOP_SECONDS       => 5,
    LOGGED_NAME_LENGTH => 80,
    MAX_NAMED_METHODS  => 20,
    SIGNAL_EXIT_BASE   => 128,
};
my %PASSED_CAPABILITIES  = map { $_ => 1 } qw(session);
my %PASSED_NOTIFICATIONS = map { $_ => 1 } ('session/update');
my %PASSED_REQUESTS      = map { $_ => 1 } ('session/request_permission');

my ($max_line, $end_seconds, $stall_seconds, $report_file, @command) = @ARGV;
die "usage: acp_proxy.pl <max line bytes> <end seconds> <stall seconds> <report file> <command...>\n"
    unless @command && !grep { !defined $_ || $_ !~ /^[0-9]+$/ } ($max_line, $end_seconds, $stall_seconds);

my $json = JSON::PP->new->utf8;
my %count = (refused => 0, refused_unnamed => 0, agent_dropped => 0, client_dropped => 0);
my %refused_method;
# Set by a signal that asks for the end.
my $stop_requested = 0;
# The wait status of the command once it has ended, and when that was seen. A command that has
# ended can leave its output open (a child in another session holds it): the relay does not
# wait for that end of the pipe, it stops a moment after the command.
my ($command_status, $command_ended_at);

# ---- messages -------------------------------------------------------------------------------

sub printable {
    my ($text) = @_;
    $text =~ s/[^\x20-\x7e]/?/g;
    return substr($text, 0, LOGGED_NAME_LENGTH);
}

sub parsed {
    my ($line) = @_;
    my $message = eval { $json->decode($line) };
    return ref $message eq 'HASH' ? $message : undef;
}

# A client line as it goes on to the agent, or nothing when it is not a JSON object.
sub client_line {
    my ($line) = @_;
    my $message = parsed($line);
    if (!$message) {
        $count{client_dropped}++;
        return;
    }
    my $params = $message->{params};
    if (($message->{method} // '') eq 'initialize' && ref $params eq 'HASH' && exists $params->{clientCapabilities}) {
        my $given = ref $params->{clientCapabilities} eq 'HASH' ? $params->{clientCapabilities} : {};
        $params->{clientCapabilities} = { map { $_ => $given->{$_} } grep { $PASSED_CAPABILITIES{$_} } keys %$given };
    }
    return $json->encode($message) . "\n";
}

# Refusals are counted by method name, for the first MAX_NAMED_METHODS names only: an agent that
# invents a name for every request must not make this process or the log grow without bound.
sub count_refused_method {
    my ($name) = @_;
    if (exists $refused_method{$name} || keys(%refused_method) < MAX_NAMED_METHODS) {
        $refused_method{$name}++;
    } else {
        $count{refused_unnamed}++;
    }
}

# What a message of the agent is: request, notification, response, or nothing valid.
sub agent_message_kind {
    my ($message) = @_;
    return 'invalid' unless $message;
    if (exists $message->{method}) {
        return 'invalid' if !defined $message->{method} || ref $message->{method};
        return exists $message->{id} ? 'request' : 'notification';
    }
    return exists $message->{id} && (exists $message->{result} || exists $message->{error}) ? 'response' : 'invalid';
}

# An agent line: returns (line for the client, line back to the agent); either may be undef.
sub agent_line {
    my ($line) = @_;
    my $message = parsed($line);
    my $kind    = agent_message_kind($message);
    my $method  = $kind eq 'request' || $kind eq 'notification' ? $message->{method} : '';
    if ($kind eq 'response' || ($kind eq 'notification' && $PASSED_NOTIFICATIONS{$method}) || ($kind eq 'request' && $PASSED_REQUESTS{$method})) {
        return ($json->encode($message) . "\n", undef);
    }
    if ($kind eq 'request') {
        $count{refused}++;
        count_refused_method(printable($method));
        print STDERR "[box] acp: refused the agent's request ", printable($method), "\n";
        my $error = { code => METHOD_NOT_FOUND, message => 'Method not available: the box does not pass this request to the client' };
        return (undef, $json->encode({ jsonrpc => '2.0', id => $message->{id}, error => $error }) . "\n");
    }
    $count{agent_dropped}++;
    return (undef, undef);
}

# ---- the command and its pipes ----------------------------------------------------------------

sub start_command {
    pipe(my $command_in, my $to_agent)   or die "pipe: $!\n";
    pipe(my $from_agent, my $command_out) or die "pipe: $!\n";
    my $pid = fork();
    die "fork: $!\n" unless defined $pid;
    if ($pid == 0) {
        close $to_agent;
        close $from_agent;
        open(STDIN,  '<&', $command_in)  or die "stdin: $!\n";
        open(STDOUT, '>&', $command_out) or die "stdout: $!\n";
        # The command gets its three standard descriptors and nothing else of this process:
        # not the lock of the front end, not the lifetime pipe.
        POSIX::close($_) for 3 .. MAX_INHERITED_FD;
        exec { $command[0] } @command or die "exec $command[0]: $!\n";
    }
    close $command_in;
    close $command_out;
    return ($pid, $to_agent, $from_agent);
}

sub set_nonblocking {
    my ($handle) = @_;
    my $flags = fcntl($handle, F_GETFL, 0) or return;
    fcntl($handle, F_SETFL, $flags | O_NONBLOCK);
}

# Writes what it can of a queue; returns 0 when the other end is gone.
sub write_some {
    my ($handle, $queue) = @_;
    my $written = syswrite($handle, $$queue);
    if (defined $written) {
        substr($$queue, 0, $written) = '';
        return 1;
    }
    return $! == EAGAIN || $! == EINTR ? 1 : 0;
}

# Takes the complete lines out of a buffer. Returns undef when a line is longer than the limit.
sub complete_lines {
    my ($buffer) = @_;
    my @lines;
    while ((my $end = index($$buffer, "\n")) >= 0) {
        return undef if $end > $max_line;
        push @lines, substr($$buffer, 0, $end);
        substr($$buffer, 0, $end + 1) = '';
    }
    return length($$buffer) > $max_line ? undef : \@lines;
}

# Why a session must end now, or nothing: a side has had bytes waiting for it for too long, or
# the agent is still there too long after the end began.
sub overdue {
    my ($queue, $progress, $ending_at) = @_;
    for my $side (qw(client agent)) {
        return "stalled $side" if length $queue->{$side} && time - $progress->{$side} > $stall_seconds;
    }
    return 'unfinished agent' if defined $ending_at && time - $ending_at > $end_seconds;
    return;
}

# Opens the lifetime pipe when box named one; nothing otherwise.
sub lifetime_handle {
    my $descriptor = $ENV{BOX_ACP_LIFETIME_FD};
    return unless defined $descriptor && $descriptor =~ /^[0-9]+$/;
    open(my $handle, '<&=', $descriptor) or return;
    set_nonblocking($handle);
    return $handle;
}

# Succeeds when the lifetime pipe has reached its end: nobody holds its writing end any more.
sub front_end_gone {
    my ($life) = @_;
    return 0 unless defined $life;
    my $read = sysread($life, my $ignored, 16);
    return defined $read && $read == 0;
}

# Read ahead within the existing client buffer while a full refusal queue pauses forwarding.
# This still observes EOF from a closed client; an agent cannot hide it by refusing the replies.
sub client_hung_up {
    my ($buffer) = @_;
    my $room = $max_line - length $$buffer;
    return 0 if $room <= 0 || !IO::Select->new(\*STDIN)->can_read(0);
    my $read = sysread(STDIN, my $more, $room < READ_BYTES ? $room : READ_BYTES);
    $$buffer .= $more if defined $read && $read > 0;
    return defined $read && $read == 0;
}

# Passes messages both ways until the agent's output ends. The end begins when the client closes
# its side or a signal arrives, and has a deadline from that moment: what the agent still sends,
# and the replies it reads, do not move it. Returns why it stopped early
# ("oversized client", "oversized agent", "stalled client", "stalled agent", "unfinished agent")
# or nothing, what was still waiting to be written, and when the end began (if it did).
sub relay {
    my ($to_agent, $from_agent, $pid) = @_;
    my %buffer = (client => '', agent => '');
    my %queue  = (client => '', agent => '');
    my %progress = (client => time, agent => time);
    my ($client_open, $agent_open, $ending_at) = (1, 1, undef);
    my $life = lifetime_handle();
    set_nonblocking($_) for $to_agent, \*STDOUT;
    while (1) {
        $ending_at //= time if $stop_requested
            || ($client_open && length($queue{agent}) > $max_line && client_hung_up(\$buffer{client}));
        return ('front end gone', \%queue, $ending_at) if front_end_gone($life);
        return (undef, \%queue, $ending_at) if command_ended($pid) && time - $command_ended_at >= DRAIN_AFTER_EXIT_SECONDS;
        my $why = overdue(\%queue, \%progress, $ending_at);
        return ($why, \%queue, $ending_at) if $why;
        my ($readers, $writers) = (IO::Select->new, IO::Select->new);
        $readers->add(\*STDIN) if $client_open && length($queue{agent}) <= $max_line;
        $readers->add($from_agent) if length($queue{client}) <= $max_line && length($queue{agent}) <= 2 * $max_line;
        $writers->add($to_agent) if $agent_open && length $queue{agent};
        $writers->add(\*STDOUT) if length $queue{client};
        my ($readable, $writable) = IO::Select->select($readers, $writers, undef, TICK_SECONDS);
        for my $handle (@{ $writable || [] }) {
            my $side = $handle == \*STDOUT ? 'client' : 'agent';
            my $waiting = length $queue{$side};
            $queue{$side} = '' unless write_some($handle, \$queue{$side});
            $progress{$side} = time if length $queue{$side} < $waiting;
        }
        for my $side (qw(client agent)) {
            $progress{$side} = time unless length $queue{$side};
        }
        for my $handle (@{ $readable || [] }) {
            my $side = $handle == \*STDIN ? 'client' : 'agent';
            my $read = sysread($handle, $buffer{$side}, READ_BYTES, length $buffer{$side});
            next if !defined $read && ($! == EAGAIN || $! == EINTR);
            if (!$read) {
                return (undef, \%queue, $ending_at) if $side eq 'agent';
                $client_open = 0;
                $ending_at //= time;
            }
            my $lines = complete_lines(\$buffer{$side}) or return ("oversized $side", \%queue, $ending_at);
            for my $line (@$lines) {
                next if $line =~ /^\s*$/;
                my ($for_client, $for_agent) = $side eq 'client' ? (undef, client_line($line)) : agent_line($line);
                $queue{client} .= $for_client if defined $for_client;
                $queue{agent}  .= $for_agent  if defined $for_agent;
            }
        }
        if (!$client_open && $agent_open && !length $queue{agent}) {
            close $to_agent;
            $agent_open = 0;
        }
    }
}

# ---- the end ----------------------------------------------------------------------------------

# Writes what the agent said last to the client, until the moment given: a client that does not
# read is not waited for. The moment is absolute and the caller's: when the end of the session
# has begun, it is the deadline of that end, which this does not move. Past it, what the client
# can take at once is still written, and nothing is waited for.
sub flush_to_client {
    my ($pending, $deadline) = @_;
    my $select = IO::Select->new(\*STDOUT);
    while (length $pending) {
        my $left = $deadline - time;
        last unless $select->can_write($left > 0 ? TICK_SECONDS : 0);
        last unless write_some(\*STDOUT, \$pending);
        last if $left <= 0;
    }
}

# Reads and drops what the command still writes, for a limited time.
sub drain {
    my ($from_agent) = @_;
    my $deadline = time + DRAIN_SECONDS;
    my $select   = IO::Select->new($from_agent);
    while (time < $deadline) {
        next unless $select->can_read(1);
        my $read = sysread($from_agent, my $dropped, READ_BYTES);
        return if defined $read && $read == 0;
    }
}

sub exit_code_of {
    my ($status) = @_;
    return $status & 127 ? SIGNAL_EXIT_BASE + ($status & 127) : $status >> 8;
}

# Succeeds once the command has ended; its status is kept for wait_for.
sub command_ended {
    my ($pid) = @_;
    return 1 if defined $command_status;
    return 0 unless waitpid($pid, POSIX::WNOHANG) == $pid;
    ($command_status, $command_ended_at) = ($?, time);
    return 1;
}

# Waits for the command; one that does not end is told to, then killed. Returns its exit code.
sub wait_for {
    my ($pid) = @_;
    return exit_code_of($command_status) if defined $command_status;
    for my $signal (undef, 'TERM', 'KILL') {
        kill $signal, $pid if $signal;
        for (1 .. STOP_SECONDS * 10) {
            return exit_code_of($?) if waitpid($pid, POSIX::WNOHANG) == $pid;
            select(undef, undef, undef, 0.1);
        }
    }
    return SIGNAL_EXIT_BASE + 9;
}

sub write_report {
    return unless $count{refused} || $count{agent_dropped} || $count{client_dropped};
    my $methods = join ', ', map { "$_: $refused_method{$_}" } sort keys %refused_method;
    $methods .= "; $count{refused_unnamed} under other names" if $count{refused_unnamed};
    open(my $report, '>', $report_file) or return;
    print $report "[box] acp: $count{refused} request(s) of the agent refused", ($methods ? " ($methods)" : ''),
        ", $count{agent_dropped} other message(s) of the agent dropped, $count{client_dropped} line(s) of the client dropped\n";
    close $report;
}

my ($pid, $to_agent, $from_agent) = start_command();
$SIG{$_} = sub { $stop_requested = 1; kill 'TERM', $pid unless defined $command_status } for qw(TERM INT HUP);
$SIG{PIPE} = 'IGNORE';
my ($why, $queue, $ending_at) = relay($to_agent, $from_agent, $pid);
close $to_agent if defined fileno($to_agent);
# A command that is still there when the session is ended early is told to end now, before
# anything else: nothing below postpones it. (It is killed if it stays; box removes the
# container after.)
kill 'TERM', $pid if $why && !defined $command_status;
# The last words of the agent get the time that is left of the end, when the end has begun (one
# deadline, counted from the moment the client closed its side), and the stall allowance when
# the agent ended by itself with the client still there.
my $flush_until = defined $ending_at ? $ending_at + $end_seconds : time + $stall_seconds;
if (!$why) {
    flush_to_client($queue->{client}, $flush_until);
} elsif ($why =~ /^oversized (\w+)$/) {
    print STDERR "[box] acp: a line of more than $max_line bytes came from the $1; the session was ended\n";
    drain($from_agent);
} elsif ($why =~ /^stalled (\w+)$/) {
    print STDERR "[box] acp: the $1 did not take what was waiting for it in $stall_seconds s; the session was ended\n";
} elsif ($why eq 'front end gone') {
    print STDERR "[box] acp: the front end is gone; the session was ended\n";
} else {
    flush_to_client($queue->{client}, $flush_until);
    print STDERR "[box] acp: the agent did not end within $end_seconds s after the conversation was closed; the session was ended\n";
}
my $code = wait_for($pid);
write_report();
exit(!$why ? $code : $why =~ /^oversized/ ? OVERSIZED_EXIT : STALLED_EXIT);
