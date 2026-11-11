#!/usr/bin/perl
# Runs a command of a project natively for box (setup, archive, a check, Play), as the person.
#
# usage: native_run.pl [--keep-stdin] <seconds> <pid file or -> <command...>
#
# The command gets a process group of its own and no standard input (its own, with
# --keep-stdin: the conversation of an agent). It ends when it ends by
# itself, when <seconds> have passed (0: no deadline) or when this process is told to end
# (TERM, INT, HUP); then the whole group is told to end and, a moment later, killed. Whatever
# the command started in its group is ended with it, also after a normal end: nothing is left
# behind. The pid file, when named, holds while the group lives a record of three lines: the id
# of the group, the boot time of the machine and the start time of the command, by which box
# knows later that the number still names this process (see recorded_group).
# Exit code: the command's own; 124 when the deadline ended it; 128 plus the signal number when
# a signal ended it or this process was told to end; 125 when the record could not be made (the
# command is then ended at once, or was never started).
use strict;
use warnings;
use POSIX qw(setpgid WNOHANG);
# The deadline is measured in fractions of a second: a whole-second clock would end the command
# up to a second early, before the time the caller will have measured.
use Time::HiRes qw(time);

use constant {
    TIMEOUT_EXIT     => 124,
    RECORD_EXIT      => 125,
    SIGNAL_EXIT_BASE => 128,
    TERM_SIGNAL      => 15,
    GRACE_SECONDS    => 3,
    TICK_SECONDS     => 0.1,
};

my $keep_stdin = @ARGV && $ARGV[0] eq '--keep-stdin' ? shift @ARGV : 0;
my ($seconds, $pid_file, @command) = @ARGV;
die "usage: native_run.pl [--keep-stdin] <seconds> <pid file or -> <command...>\n"
    unless @command && defined $seconds && $seconds =~ /^[0-9]+$/;

sub group_lives {
    my ($group) = @_;
    return kill(0, -$group) ? 1 : 0;
}

# Tells the group to end, waits a moment for it, then kills what is left.
sub end_group {
    my ($group) = @_;
    return unless group_lives($group);
    kill 'TERM', -$group;
    for (1 .. GRACE_SECONDS / TICK_SECONDS) {
        waitpid(-1, WNOHANG);
        return unless group_lives($group);
        select(undef, undef, undef, TICK_SECONDS);
    }
    kill 'KILL', -$group;
}

# The boot of the machine and the start of a process, for the record: read with programs named
# by their absolute paths (a short PATH must not leave them empty), from /proc where there is
# no such program. Nothing is returned when a value cannot be read.
sub boot_identity {
    if (-r '/proc/sys/kernel/random/boot_id') {
        open(my $file, '<', '/proc/sys/kernel/random/boot_id') or return;
        my $id = <$file> // '';
        $id =~ s/\s+//g;
        return $id;
    }
    my ($seconds) = `/usr/sbin/sysctl -n kern.boottime 2>/dev/null` =~ /^\{ sec = ([0-9]+),/;
    return $seconds;
}

sub start_identity {
    my ($process) = @_;
    if (-r "/proc/$process/stat") {
        open(my $file, '<', "/proc/$process/stat") or return;
        my $line = <$file> // '';
        $line =~ s/^.*\)\s+//;
        return (split ' ', $line)[19];
    }
    local $ENV{LC_ALL} = 'C';
    my $start = `/bin/ps -p $process -o lstart= 2>/dev/null`;
    $start =~ s/^\s+|\s+$//g;
    return $start;
}

# A command that was started but cannot be recorded is ended and collected at once: nothing may
# run that box could not find again.
sub abandon {
    my ($process, $reason) = @_;
    kill 'KILL', -$process;
    kill 'KILL', $process;
    waitpid($process, 0);
    print STDERR "[box] $reason; the command was ended\n";
    exit RECORD_EXIT;
}

my $boot;
if ($pid_file ne '-') {
    $boot = boot_identity();
    die "[box] the boot time of the machine cannot be read, so no record of the command can be kept; it was not started\n"
        unless defined $boot && length $boot;
}
my $pid = fork();
die "fork: $!\n" unless defined $pid;
if ($pid == 0) {
    setpgid(0, 0);
    open(STDIN, '<', '/dev/null') unless $keep_stdin;
    exec { $command[0] } @command or exit 127;
}
setpgid($pid, $pid);
if ($pid_file ne '-') {
    my $start = start_identity($pid);
    abandon($pid, 'the start time of the command cannot be read, so no record of it can be kept')
        unless defined $start && length $start;
    my $file;
    abandon($pid, "the record of the command cannot be written ($pid_file: $!)")
        unless open($file, '>', "$pid_file.tmp") && print($file "$pid\n$boot\n$start\n") && close($file)
        && rename("$pid_file.tmp", $pid_file);
}

my ($why, $status) = ('', undef);
$SIG{$_} = sub { $why ||= 'signal' } for qw(TERM INT HUP);
my $deadline = $seconds > 0 ? time + $seconds : undef;
until ($why) {
    if (waitpid($pid, WNOHANG) == $pid) { $status = $?; last; }
    $why = 'deadline' if defined $deadline && time >= $deadline;
    select(undef, undef, undef, TICK_SECONDS) unless $why;
}
end_group($pid);
waitpid($pid, 0) unless defined $status;
unlink $pid_file if $pid_file ne '-';
exit TIMEOUT_EXIT if $why eq 'deadline';
exit SIGNAL_EXIT_BASE + TERM_SIGNAL if $why eq 'signal';
exit($status & 127 ? SIGNAL_EXIT_BASE + ($status & 127) : $status >> 8);
