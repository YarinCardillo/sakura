#!/usr/bin/perl
# Copies standard input to standard output up to a number of bytes, then keeps reading and
# drops the rest: whoever floods a log neither fills the disk nor is stopped by a full pipe, and
# a last line says how much was dropped.
#
# usage: bounded_log.pl <bytes> [<done file> <drain seconds> [<pipe> <rendezvous seconds>]]
#
# Without a done file it reads until the input ends. With one, the caller creates that file
# when the command whose output this is has ended; from then on this reads for <drain seconds>
# more and stops, whoever still holds the other end of the pipe (a child the command left in
# another session keeps it open for as long as it lives, and nobody waits for that).
#
# With a pipe, the input is that named pipe and not standard input. Opening it waits until
# somebody opens it for writing; this is done here, and not by the shell that starts this
# program, so that nothing the shell would still hold (a lock of box) is held during the wait.
# The wait is bounded: when no writer has come after <rendezvous seconds> this program is ended
# by the alarm it set.
use strict;
use warnings;
use IO::Select;

use constant TICK_SECONDS => 0.2;

my ($limit, $done_file, $drain_seconds, $pipe, $rendezvous_seconds) = @ARGV;
die "usage: bounded_log.pl <bytes> [<done file> <drain seconds> [<pipe> <rendezvous seconds>]]\n"
    unless defined $limit && $limit =~ /^[0-9]+$/ && (!defined $done_file || (defined $drain_seconds && $drain_seconds =~ /^[0-9]+$/))
    && (!defined $pipe || (defined $rendezvous_seconds && $rendezvous_seconds =~ /^[1-9][0-9]*$/));

if (defined $pipe) {
    # No handler for the alarm: it ends this program, which is what a writer that never comes calls for.
    alarm $rendezvous_seconds;
    open(STDIN, '<', $pipe) or die "bounded_log.pl: cannot open $pipe: $!\n";
    alarm 0;
}

my ($kept, $dropped, $done_at) = (0, 0, undef);
binmode STDIN;
binmode STDOUT;
$| = 1;
my $input = IO::Select->new(\*STDIN);
while (1) {
    if (defined $done_file) {
        $done_at //= time if -e $done_file;
        last if defined $done_at && time - $done_at >= $drain_seconds;
        next unless $input->can_read(TICK_SECONDS);
    }
    my $read = sysread(STDIN, my $chunk, 65536);
    last unless $read;
    my $part = substr($chunk, 0, $limit - $kept);
    print $part;
    $kept += length $part;
    $dropped += $read - length $part;
}
print "\n[box] log cut at $limit bytes, $dropped more bytes dropped\n" if $dropped > 0;
